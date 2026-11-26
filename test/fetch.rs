use bgdeobf::error::{Error, Result};
use bgdeobf::{fetch, paste_envelope};
use fetch::{HttpReply, Transport};
use serenity::async_trait;
use std::net::IpAddr;
use std::{collections::HashMap, io::Write, net::SocketAddr, sync::Mutex, time::Duration};
use url::Url;

#[test]
fn public_addresses_and_mixed_dns_answers() {
    for (ip, public) in [
        ("8.8.8.8", true),
        ("1.1.1.1", true),
        ("93.184.216.34", true),
        ("2606:4700:4700::1111", true),
        ("4000::1", true),
        ("::2", true),
        ("::ffff:8.8.8.8", true),
        ("127.0.0.1", false),
        ("127.1.2.3", false),
        ("0.0.0.0", false),
        ("10.0.0.5", false),
        ("172.16.5.5", false),
        ("10.0.0.1", false),
        ("172.31.255.255", false),
        ("192.168.1.1", false),
        ("100.64.0.1", false),
        ("169.254.169.254", false),
        ("198.18.0.1", false),
        ("192.0.2.1", false),
        ("224.0.0.1", false),
        ("255.255.255.255", false),
        ("::1", false),
        ("::", false),
        ("fe80::1", false),
        ("fc00::1", false),
        ("fd12:3456::1", false),
        ("::ffff:127.0.0.1", false),
        ("::ffff:10.1.1.1", false),
        ("64:ff9b::7f00:1", false),
        ("2002:7f00:1::", false),
        ("2001:db8::1", false),
        ("ff02::1", false),
        ("3fff::1", false),
        ("fec0::1", false),
    ] {
        assert_eq!(
            fetch::is_public(ip.parse::<IpAddr>().unwrap()),
            public,
            "{ip}"
        );
    }
    assert!(fetch::validate_addresses(&[
        "8.8.8.8:443".parse().unwrap(),
        "127.0.0.1:443".parse().unwrap()
    ])
    .is_err());
    assert!(fetch::validate_addresses(&[]).is_err());
}

#[test]
fn exact_reference_network_policy_includes_all_ipv6_denials_without_new_allowlist() {
    // Reference policy is fetch.py's explicit blocks plus Python 3.14 ipaddress.is_global.
    for (ip, public) in [
        ("::2", true),
        ("4000::1", true),
        ("64:ff9b::1", false),
        ("64:ff9b:1::1", false),
        ("100::1", false),
        ("2001:1::1", false),
        ("2001:3::1", false),
        ("2001:20::1", false),
        ("2001:200::1", true),
        ("3fff::1", false),
        ("3fff:fff:ffff:ffff:ffff:ffff:ffff:ffff", false),
        ("3fff:1000::1", true),
        ("fc00::1", false),
        ("fdff:ffff::1", false),
        ("fe80::1", false),
        ("febf:ffff::1", false),
        ("fec0::1", false),
        ("feff:ffff::1", false),
        ("ff00::1", false),
        ("ffff:ffff::1", false),
        ("192.0.0.9", false),
        ("192.88.99.1", false),
        ("198.51.100.1", false),
        ("203.0.113.1", false),
    ] {
        assert_eq!(fetch::is_public(ip.parse().unwrap()), public, "{ip}");
    }
}

#[test]
fn urls_normalization_and_restrictions() {
    assert_eq!(
        fetch::normalize_url("https://github.com/u/r/blob/main/dir/x.lua"),
        "https://raw.githubusercontent.com/u/r/main/dir/x.lua"
    );
    assert_eq!(
        fetch::normalize_url("https://pastebin.com/AbCd1234"),
        "https://pastebin.com/raw/AbCd1234"
    );
    assert_eq!(
        fetch::normalize_url("https://gist.github.com/u/0123456789abcdef"),
        "https://gist.githubusercontent.com/u/0123456789abcdef/raw"
    );
    assert_eq!(
        fetch::extract_url("loadstring(game:HttpGet(\"https://x.io/a.lua\"))()"),
        Some("https://x.io/a.lua".into())
    );
    assert_eq!(
        fetch::url_stem("https://h/My%20Script.lua?x=1"),
        "My%20Script"
    );
    for u in [
        "ftp://x.io/x",
        "file:///etc/passwd",
        "https://u:p@x.io/x",
        "https://@x.io/x",
        "https://x.io:22/x",
        "http://127.1/",
        "http://2130706433/",
        "http://[::ffff:7f00:1]/",
        "http://[fe80::1]/",
    ] {
        assert!(fetch::check_url(u, &[]).is_err(), "{u}");
    }
    assert!(fetch::check_url(
        "https://raw.githubusercontent.com/x",
        &["githubusercontent.com".into()]
    )
    .is_ok());
    assert!(fetch::check_url(
        "https://notgithubusercontent.com/x",
        &["githubusercontent.com".into()]
    )
    .is_err());
}

#[test]
fn envelope_origin_ambiguity_and_encoded_values() {
    let page = "https://PASTE.example.:443/raw/x";
    let link = "https://paste.example/api/stream/AbC123xyz?k=fake";
    let plain = |s: &str| {
        format!("-- Secure Runtime Envelope\nlocal url = \"{s}\"\nreturn game:HttpGet(url)")
    };
    assert_eq!(
        paste_envelope::stream_url(
            plain("https://paste.example/api/stream/z/../x").as_bytes(),
            page
        )
        .unwrap(),
        None
    );
    assert_eq!(
        paste_envelope::stream_url(plain(link).as_bytes(), page).unwrap(),
        Some(link.into())
    );
    for bad in [
        "https://evil.example/api/stream/x",
        "http://paste.example/api/stream/x",
        "https://paste.example:8443/api/stream/x",
        "https://u:p@paste.example/api/stream/x",
        "https://paste.example/raw/x",
        "https://paste.example/api/stream/x#f",
        "https://paste.example/api/stream/x\\q",
    ] {
        assert_eq!(
            paste_envelope::stream_url(plain(bad).as_bytes(), page).unwrap(),
            None,
            "{bad}"
        );
    }
    assert_eq!(
        paste_envelope::stream_url(format!("{}{}", plain(link), plain(link)).as_bytes(), page)
            .unwrap(),
        None
    );
    for (key, step) in [(42u32, 10u32), (146, 16), (0, 1), (255, 255)] {
        let hex = link
            .bytes()
            .enumerate()
            .map(|(i, b)| format!("{:02x}", (u32::from(b) + key + i as u32 * step) % 256))
            .collect::<String>();
        let text=format!("local g,p,k,i,t,c,h=game,pcall,{key},0,{{}},string.char,\"{hex}\";h:gsub(\"..\",function(x)t[i+1]=c((tonumber(x,16)-(k+i*{step}))%256)i=i+1 end);g:HttpGet(table.concat(t))");
        assert_eq!(
            paste_envelope::stream_url(text.as_bytes(), page).unwrap(),
            Some(link.into())
        );
    }
}

#[test]
fn loader_binary_size_marker_and_ambiguous_candidates() {
    let page = "https://paste.example/raw/x";
    for data in [
        Vec::new(),
        b" \t\r\n\x0b\x0c".to_vec(),
        b"\xff\xfe\0".to_vec(),
        vec![b'x'; 8193],
        b"print(1)".to_vec(),
        b"local url='https://paste.example/api/stream/x'".to_vec(),
    ] {
        assert_eq!(paste_envelope::stream_url(&data, page).unwrap(), None);
    }
    let loader=b"\xef\xbb\xbf-- Secure Runtime Envelope\r\nlocal url='https://paste.example/api/stream/x?k=1'\r\nreturn game:HttpGet(url)";
    assert!(paste_envelope::stream_url(loader, page).unwrap().is_some());
    for bad_page in ["", "http://[::1", "file:///x"] {
        assert_eq!(paste_envelope::stream_url(loader, bad_page).unwrap(), None);
    }
    let encoded="local g,p,k,i,t,c,h=game,pcall,١,0,{},string.char,\"aa\";h:gsub(\"..\",function(x)t[i+1]=c((tonumber(x,16)-(k+i*١))%256)i=i+1 end);g:HttpGet(table.concat(t))";
    assert_eq!(
        paste_envelope::stream_url(encoded.as_bytes(), page).unwrap(),
        None
    );
}

#[derive(Default)]
struct Net {
    answers: HashMap<String, HttpReply>,
    errors: HashMap<String, (&'static str, &'static str)>,
    dns: HashMap<String, Vec<SocketAddr>>,
    dns_errors: HashMap<String, (&'static str, &'static str)>,
    calls: Mutex<Vec<(String, Vec<SocketAddr>, String)>>,
    resolutions: Mutex<Vec<String>>,
    budgets: Mutex<Vec<(usize, Duration)>>,
    delay: bool,
    dns_delay: bool,
    response_delay: Duration,
    slow_url: Option<String>,
}
#[async_trait]
impl Transport for Net {
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>> {
        self.resolutions.lock().unwrap().push(host.into());
        if let Some((code, message)) = self.dns_errors.get(host) {
            return Err(Error::new(*code, *message));
        }
        if self.dns_delay {
            std::future::pending::<()>().await;
        }
        Ok(self
            .dns
            .get(host)
            .cloned()
            .unwrap_or_else(|| vec![SocketAddr::new("8.8.8.8".parse().unwrap(), port)]))
    }
    async fn get(
        &self,
        url: &Url,
        addresses: &[SocketAddr],
        max: usize,
        timeout: Duration,
        agent: &str,
    ) -> Result<HttpReply> {
        self.calls
            .lock()
            .unwrap()
            .push((url.to_string(), addresses.to_vec(), agent.into()));
        self.budgets.lock().unwrap().push((max, timeout));
        if !self.response_delay.is_zero() {
            tokio::time::sleep(self.response_delay).await;
        }
        if self.delay || self.slow_url.as_deref() == Some(url.as_str()) {
            std::future::pending::<()>().await;
        }
        if let Some((code, message)) = self.errors.get(url.as_str()) {
            return Err(Error::new(*code, *message));
        }
        let Some(a) = self.answers.get(url.as_str()) else {
            return Err(Error::new("net", "could not connect"));
        };
        Ok(HttpReply {
            status: a.status,
            location: a.location.clone(),
            encoding: a.encoding.clone(),
            content_type: a.content_type.clone(),
            body: a.body.clone(),
        })
    }
}
fn reply(body: impl Into<Vec<u8>>) -> HttpReply {
    HttpReply {
        status: 200,
        body: body.into(),
        ..Default::default()
    }
}
fn redirect(location: &str) -> HttpReply {
    HttpReply {
        status: 302,
        location: Some(location.into()),
        ..Default::default()
    }
}
async fn get(net: &Net, url: &str, max: usize) -> Result<Vec<u8>> {
    fetch::fetch_with(url, max, &[], Duration::from_secs(1), net).await
}

#[tokio::test]
async fn redirects_use_validated_addresses_and_current_host_headers() {
    let mut net = Net::default();
    let start = "https://public.test/x";
    let target = format!("https://{}/x", fetch::ROBLOX_PASTE_HOST);
    net.answers.insert(start.into(), redirect(&target));
    net.answers.insert(target.clone(), reply(b"print(1)"));
    assert_eq!(get(&net, start, 1000).await.unwrap(), b"print(1)");
    let calls = net.calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].2, fetch::USER_AGENT);
    assert_eq!(calls[1].2, "Roblox/WinInet");
    assert_eq!(calls[0].1, vec!["8.8.8.8:443".parse().unwrap()]);
    assert_eq!(net.resolutions.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn mixed_answers_are_blocked_before_any_connection() {
    for private in [
        "127.0.0.1:443",
        "169.254.169.254:443",
        "[::1]:443",
        "[::ffff:10.0.0.1]:443",
    ] {
        let mut net = Net::default();
        net.dns.insert(
            "mixed.test".into(),
            vec!["8.8.8.8:443".parse().unwrap(), private.parse().unwrap()],
        );
        assert_eq!(
            get(&net, "https://mixed.test/x", 1000)
                .await
                .unwrap_err()
                .code,
            "blocked"
        );
        assert!(net.calls.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn loopback_spellings_and_redirect_dns_never_reach_the_transport() {
    for host in [
        "localhost",
        "localhost.localdomain",
        "0x7f000001",
        "127.1",
        "2130706433",
    ] {
        let net = Net {
            dns: HashMap::from([(host.into(), vec!["127.0.0.1:443".parse().unwrap()])]),
            ..Default::default()
        };
        assert_eq!(
            get(&net, &format!("https://{host}/x"), 1000)
                .await
                .unwrap_err()
                .code,
            "blocked",
            "{host}"
        );
        assert!(net.calls.lock().unwrap().is_empty(), "{host}");
    }
    for addresses in [
        vec!["10.0.0.1:443".parse().unwrap()],
        vec![
            "8.8.8.8:443".parse().unwrap(),
            "[::ffff:127.0.0.1]:443".parse().unwrap(),
        ],
    ] {
        let net = Net {
            answers: HashMap::from([(
                "https://public.test/x".into(),
                redirect("https://private.test/x"),
            )]),
            dns: HashMap::from([("private.test".into(), addresses)]),
            ..Default::default()
        };
        assert_eq!(
            get(&net, "https://public.test/x", 1000)
                .await
                .unwrap_err()
                .code,
            "blocked"
        );
        assert_eq!(
            net.resolutions.lock().unwrap().as_slice(),
            ["public.test", "private.test"]
        );
        let calls = net.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "https://public.test/x");
    }
}

#[tokio::test]
async fn dns_failures_stop_before_get_and_public_answers_keep_their_order_and_port() {
    for resolver_error in [false, true] {
        let mut net = Net::default();
        if resolver_error {
            net.dns_errors
                .insert("public.test".into(), ("net", "controlled DNS failure"));
        } else {
            net.dns.insert("public.test".into(), Vec::new());
        }
        let error = get(&net, "https://public.test/x", 1000).await.unwrap_err();
        assert_eq!(error.code, "net");
        assert_eq!(
            error.message,
            if resolver_error {
                "controlled DNS failure"
            } else {
                "could not resolve that server"
            }
        );
        assert!(net.calls.lock().unwrap().is_empty());
        assert_eq!(net.resolutions.lock().unwrap().as_slice(), ["public.test"]);
    }
    for (url, port) in [("http://public.test/x", 80), ("https://public.test/x", 443)] {
        let addresses = vec![
            SocketAddr::new("2606:4700:4700::1111".parse().unwrap(), port),
            SocketAddr::new("1.1.1.1".parse().unwrap(), port),
        ];
        let net = Net {
            answers: HashMap::from([(url.into(), reply(b"print('ordinary script')"))]),
            dns: HashMap::from([("public.test".into(), addresses.clone())]),
            ..Default::default()
        };
        assert_eq!(
            get(&net, url, 1000).await.unwrap(),
            b"print('ordinary script')"
        );
        assert_eq!(net.resolutions.lock().unwrap().as_slice(), ["public.test"]);
        let calls = net.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, url);
        assert_eq!(calls[0].1, addresses);
    }
}

#[test]
fn gzip_keeps_the_reference_first_member_without_decoding_trailing_members() {
    let mut first = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    first.write_all(b"print('first member')\n").unwrap();
    let first = first.finish().unwrap();
    let mut second = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    second.write_all(&vec![b'x'; 4096]).unwrap();
    let second = second.finish().unwrap();
    for suffix in [&second[..], &b"trailing unused bytes"[..]] {
        let mut body = first.clone();
        body.extend_from_slice(suffix);
        assert!(body.len() < 1024);
        for encoding in ["gzip", "x-gzip"] {
            assert_eq!(
                fetch::decode_body(&body, encoding, "text/plain", 1024).unwrap(),
                b"print('first member')\n"
            );
        }
    }
}

#[tokio::test]
async fn every_redirect_is_checked_again() {
    for (location, code) in [
        ("http://127.0.0.1/", "blocked"),
        ("http://[::1]/", "blocked"),
        ("file:///etc/passwd", "bad_url"),
        ("https://x.test:8443/", "bad_url"),
        ("https://u:p@x.test/", "bad_url"),
    ] {
        let net = Net {
            answers: HashMap::from([("https://public.test/x".into(), redirect(location))]),
            ..Default::default()
        };
        assert_eq!(
            get(&net, "https://public.test/x", 1000)
                .await
                .unwrap_err()
                .code,
            code
        );
        assert_eq!(net.calls.lock().unwrap().len(), 1);
    }
    let net = Net {
        answers: HashMap::from([(
            "https://public.test/x".into(),
            redirect("https://other.test/x"),
        )]),
        ..Default::default()
    };
    assert_eq!(
        fetch::fetch_with(
            "https://public.test/x",
            1000,
            &["public.test".into()],
            Duration::from_secs(1),
            &net
        )
        .await
        .unwrap_err()
        .code,
        "blocked"
    );
    let net = Net {
        answers: HashMap::from([("https://public.test/x".into(), redirect("/x"))]),
        ..Default::default()
    };
    assert_eq!(
        get(&net, "https://public.test/x", 1000)
            .await
            .unwrap_err()
            .code,
        "http"
    );
    assert_eq!(net.calls.lock().unwrap().len(), 4);
}

#[test]
fn compressed_and_expanded_limits_are_separate() {
    let data = b"print(1)\n";
    for encoding in ["gzip", "x-gzip", "deflate"] {
        let encode = |bytes: &[u8]| {
            if encoding == "deflate" {
                let mut z =
                    flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
                z.write_all(bytes).unwrap();
                z.finish().unwrap()
            } else {
                let mut z = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
                z.write_all(bytes).unwrap();
                z.finish().unwrap()
            }
        };
        let encoded = encode(data);
        assert_eq!(
            fetch::decode_body(&encoded, encoding, "text/plain", 1000).unwrap(),
            data
        );
        assert_eq!(
            fetch::decode_body(&encoded, encoding, "", 1)
                .unwrap_err()
                .code,
            "too_big"
        );
        assert_eq!(
            fetch::decode_body(b"corrupt", encoding, "", 1000)
                .unwrap_err()
                .code,
            "http"
        );
        for whitespace in *b"\t\n\x0b\x0c\r " {
            assert_eq!(
                fetch::decode_body(&encode(&[whitespace]), encoding, "", 1000)
                    .unwrap_err()
                    .code,
                "empty"
            );
        }
        assert_eq!(
            fetch::decode_body(&encode(b"\x08"), encoding, "", 1000).unwrap(),
            b"\x08"
        );
    }
    let mut z = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    z.write_all(&vec![0; 100000]).unwrap();
    let bomb = z.finish().unwrap();
    assert_eq!(
        fetch::decode_body(&bomb, "gzip", "", 1000)
            .unwrap_err()
            .code,
        "too_big"
    );
    assert_eq!(
        fetch::decode_body(b" ", "identity", "", 1000)
            .unwrap_err()
            .code,
        "empty"
    );
    assert_eq!(
        fetch::decode_body(b"\t\n\x0b\x0c\r ", "identity", "", 1000)
            .unwrap_err()
            .code,
        "empty"
    );
    assert_eq!(
        fetch::decode_body(b"<!DOCTYPE html><html>", "identity", "text/plain", 1000)
            .unwrap_err()
            .code,
        "html"
    );
    assert_eq!(
        fetch::decode_body(data, "identity", "text/html", 1000).unwrap(),
        data
    );
    assert_eq!(
        fetch::decode_body(b"zzz", "br", "", 1000).unwrap_err().code,
        "http"
    );
}

#[tokio::test]
async fn envelope_is_one_followup_and_preserves_error_distinctions() {
    let page = "https://paste.example/raw/x";
    let link = "https://paste.example/api/stream/x?k=fake";
    let loader = format!("-- Secure Runtime Envelope\nlocal url=\"{link}\"\ngame:HttpGet(url)");
    let net = Net {
        answers: HashMap::from([
            (page.into(), reply(loader.as_bytes())),
            (link.into(), reply(b"print(1)")),
        ]),
        ..Default::default()
    };
    assert_eq!(get(&net, page, 1000).await.unwrap(), b"print(1)");
    assert_eq!(net.calls.lock().unwrap().len(), 2);
    let net = Net {
        answers: HashMap::from([
            (page.into(), reply(loader.as_bytes())),
            (link.into(), reply(loader.as_bytes())),
        ]),
        ..Default::default()
    };
    assert_eq!(get(&net, page, 1000).await.unwrap_err().code, "loader");
    assert_eq!(net.calls.lock().unwrap().len(), 2);
    let net = Net {
        answers: HashMap::from([
            (page.into(), reply(loader.as_bytes())),
            (
                link.into(),
                HttpReply {
                    status: 403,
                    ..Default::default()
                },
            ),
        ]),
        ..Default::default()
    };
    let error = get(&net, page, 1000).await.unwrap_err();
    assert_eq!(error.code, "loader");
    assert!(error.message.contains("HTTP 403"));
    let net = Net {
        answers: HashMap::from([
            (page.into(), reply(loader.as_bytes())),
            (link.into(), reply(vec![b'x'; 1001])),
        ]),
        ..Default::default()
    };
    assert_eq!(get(&net, page, 1000).await.unwrap_err().code, "too_big");
}

#[tokio::test]
async fn timeout_covers_dns_download_and_followup() {
    let net = Net {
        delay: true,
        ..Default::default()
    };
    assert_eq!(
        fetch::fetch_with(
            "https://public.test/x",
            1000,
            &[],
            Duration::from_millis(10),
            &net
        )
        .await
        .unwrap_err()
        .code,
        "timeout"
    );
    let page = "https://paste.example/raw/x";
    let link = "https://paste.example/api/stream/x?k=fake";
    let net = Net {
        answers: HashMap::from([(
            page.into(),
            reply(
                format!("-- Secure Runtime Envelope\nlocal url=\"{link}\"\ngame:HttpGet(url)")
                    .into_bytes(),
            ),
        )]),
        slow_url: Some(link.into()),
        ..Default::default()
    };
    assert_eq!(
        fetch::fetch_with(page, 1000, &[], Duration::from_millis(10), &net)
            .await
            .unwrap_err()
            .code,
        "loader"
    );
}

#[tokio::test]
async fn custom_agent_and_reverse_redirect_headers() {
    let target = "https://public.test/x";
    let start = format!("https://{}/x", fetch::ROBLOX_PASTE_HOST);
    let net = Net {
        answers: HashMap::from([
            (start.clone(), redirect(target)),
            (target.into(), reply(b"print(1)")),
        ]),
        ..Default::default()
    };
    fetch::fetch_with_agent(
        &start,
        1000,
        &[],
        Duration::from_secs(1),
        &net,
        "custom-client/1",
    )
    .await
    .unwrap();
    assert!(net
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|c| c.2 == "custom-client/1"));
}

#[test]
fn extract_normalize_and_rejected_url_assertions_from_reference() {
    for (text, want) in [
        ("!uwu https://a.b/c.lua", Some("https://a.b/c.lua")),
        (
            "loadstring(game:HttpGet(\"https://raw.githubusercontent.com/u/r/main/x.lua\"))()",
            Some("https://raw.githubusercontent.com/u/r/main/x.lua"),
        ),
        (
            "loadstring(game:HttpGet('https://x.io/y.lua', true))()",
            Some("https://x.io/y.lua"),
        ),
        ("see <https://x.io/y.lua>.", Some("https://x.io/y.lua")),
        (
            "```lua\nlocal u = \"http://h/p.lua\"\n```",
            Some("http://h/p.lua"),
        ),
        ("no url here", None),
        ("", None),
    ] {
        assert_eq!(fetch::extract_url(text).as_deref(), want);
    }
    for (url, want) in [
        (
            "https://pastebin.com/raw/AbCd1234",
            "https://pastebin.com/raw/AbCd1234",
        ),
        (
            "https://gist.github.com/user/0123456789abcdef0123456789abcdef",
            "https://gist.githubusercontent.com/user/0123456789abcdef0123456789abcdef/raw",
        ),
        ("https://example.com/a.lua", "https://example.com/a.lua"),
    ] {
        assert_eq!(fetch::normalize_url(url), want);
    }
    assert_eq!(fetch::url_stem("https://h/"), "script");
    for (url, code) in [
        ("ftp://example.com/x", "bad_url"),
        ("file:///etc/passwd", "bad_url"),
        ("javascript:alert(1)", "bad_url"),
        ("https://user:pw@example.com/x", "bad_url"),
        ("https://example.com:8443/x", "bad_url"),
        ("https://example.com:22/x", "bad_url"),
        ("http://", "bad_url"),
        ("http://[::1", "bad_url"),
        ("http://127.0.0.1/", "blocked"),
        ("http://10.1.2.3/x", "blocked"),
        ("http://[::1]/", "blocked"),
        ("http://169.254.169.254/latest/meta-data/", "blocked"),
        ("http://[::ffff:7f00:1]/", "blocked"),
        ("http://[fe80::1]/", "blocked"),
        ("http://0.0.0.0/", "blocked"),
    ] {
        assert_eq!(fetch::check_url(url, &[]).unwrap_err().code, code, "{url}");
    }
    assert!(fetch::check_url("http://example.com:80/x", &[]).is_ok());
    assert_eq!(
        fetch::check_url("https://evil.example/x", &["githubusercontent.com".into()])
            .unwrap_err()
            .code,
        "blocked"
    );
}

fn envelope(link: &str) -> Vec<u8> {
    format!("-- Secure Runtime Envelope\nlocal url=\"{link}\"\ngame:HttpGet(url)").into_bytes()
}
fn encoded_envelope(link: &str) -> Vec<u8> {
    let hex = link
        .bytes()
        .enumerate()
        .map(|(i, b)| format!("{:02x}", (usize::from(b) + 146 + i * 16) % 256))
        .collect::<String>();
    format!("local g,p,k,i,t,c,h=game,pcall,146,0,{{}},string.char,\"{hex}\";h:gsub(\"..\",function(x)t[i+1]=c((tonumber(x,16)-(k+i*16))%256)i=i+1 end);g:HttpGet(table.concat(t))").into_bytes()
}

#[test]
fn every_plain_and_encoded_envelope_restriction_and_malformed_decoder() {
    let page = "https://paste.example/raw/x";
    let link = "https://paste.example/api/stream/AbC123xyz?k=a.b-c_d==&x=1";
    assert_eq!(
        paste_envelope::stream_url(&envelope(link), page)
            .unwrap()
            .as_deref(),
        Some(link)
    );
    let custom = "http://paste.example:8080/api/stream/x?k=1";
    assert_eq!(
        paste_envelope::stream_url(&envelope(custom), "http://paste.example:8080/raw/x")
            .unwrap()
            .as_deref(),
        Some(custom)
    );
    for bad in [
        "https://evil.example/api/stream/x?k=1",
        "https://paste.example.evil.example/api/stream/x?k=1",
        "https://sub.paste.example/api/stream/x?k=1",
        "http://paste.example/api/stream/x?k=1",
        "https://paste.example:8443/api/stream/x?k=1",
        "https://user:pw@paste.example/api/stream/x?k=1",
        "https://127.0.0.1/api/stream/x?k=1",
        "https://paste.example/raw/x",
        "https://paste.example/api/stream",
        "https://paste.example/api/stream/",
        "https://paste.example/api/streams/x",
        "https://paste.example/api/stream/../../raw/x",
        "https://paste.example/api/stream/x#frag",
        "https://paste.example/api/stream/x\\q",
        "https://paste.example/api/stream/x?k=a\\b",
        "https://paste.example/api/stream/x y",
        "https://paste.example/api/stream/x?k=a\tb",
        "file:///etc/passwd",
        "//paste.example/api/stream/x",
    ] {
        for data in [envelope(bad), encoded_envelope(bad)] {
            assert_eq!(
                paste_envelope::stream_url(&data, page).unwrap(),
                None,
                "{bad}"
            );
        }
    }
    let encoded = String::from_utf8(encoded_envelope(link)).unwrap();
    for broken in [
        encoded.replacen("string.char,\"", "string.char,\"z", 1),
        encoded.replacen("string.char,\"", "string.char,\"f", 1),
        encoded.replace("tonumber(x,16)", "tonumber(x,10)"),
        encoded.replace("g:HttpGet(table.concat(t))", "nil"),
        encoded.repeat(2),
    ] {
        assert_eq!(
            paste_envelope::stream_url(broken.as_bytes(), page).unwrap(),
            None
        );
    }
    for data in [
        envelope(link).repeat(2),
        envelope(link).into_iter().chain(vec![b'x'; 8192]).collect(),
        b"\x1bLuaQ\0\x01\x02"
            .iter()
            .copied()
            .chain(0u8..=255)
            .collect(),
        String::from_utf8(envelope(link))
            .unwrap()
            .replace("Secure Runtime Envelope", "Something Else")
            .into_bytes(),
        String::from_utf8(envelope(link))
            .unwrap()
            .replace("game:HttpGet(url)", "nil")
            .into_bytes(),
    ] {
        assert_eq!(paste_envelope::stream_url(&data, page).unwrap(), None);
    }
}

#[tokio::test]
async fn envelope_followup_retains_all_error_codes_limits_and_headers() {
    let page = "https://paste.example/raw/x";
    let link = "https://paste.example/api/stream/x?k=1";
    for encoded in [false, true] {
        let net = Net {
            answers: HashMap::from([
                (
                    page.into(),
                    reply(if encoded {
                        encoded_envelope(link)
                    } else {
                        envelope(link)
                    }),
                ),
                (link.into(), reply(b"print(1)")),
            ]),
            ..Default::default()
        };
        assert_eq!(
            fetch::fetch_with_agent(
                page,
                1234,
                &["paste.example".into()],
                Duration::from_secs(7),
                &net,
                "me/1"
            )
            .await
            .unwrap(),
            b"print(1)"
        );
        {
            let calls = net.calls.lock().unwrap();
            assert_eq!(calls.len(), 2);
            assert_eq!(calls[0].0, page);
            assert_eq!(calls[1].0, link);
            assert!(calls.iter().all(|c| c.2 == "me/1"));
        }
        for second in [
            envelope(link),
            envelope("https://paste.example/api/stream/other"),
            encoded_envelope(link),
        ] {
            let net = Net {
                answers: HashMap::from([
                    (page.into(), reply(envelope(link))),
                    (link.into(), reply(second)),
                ]),
                ..Default::default()
            };
            assert_eq!(get(&net, page, 1000).await.unwrap_err().code, "loader");
            assert_eq!(net.calls.lock().unwrap().len(), 2);
        }
    }
    for (second, detail) in [
        (
            HttpReply {
                status: 403,
                ..Default::default()
            },
            "HTTP 403",
        ),
        (reply(b"<!doctype html>"), "web page"),
        (reply(b" \n"), "empty"),
        (redirect("http://127.0.0.1/"), "not a public address"),
        (redirect("file:///etc/passwd"), "only http:// or https://"),
    ] {
        let net = Net {
            answers: HashMap::from([(page.into(), reply(envelope(link))), (link.into(), second)]),
            ..Default::default()
        };
        let error = get(&net, page, 1000).await.unwrap_err();
        assert_eq!(error.code, "loader");
        assert!(error.message.contains(detail), "{}", error.message);
        assert_eq!(net.calls.lock().unwrap().len(), 2);
    }
    let net = Net {
        answers: HashMap::from([(page.into(), reply(envelope(link)))]),
        ..Default::default()
    };
    assert_eq!(get(&net, page, 1000).await.unwrap_err().code, "loader");
    let other = envelope("https://evil.example/api/stream/x");
    let net = Net {
        answers: HashMap::from([(page.into(), reply(other.clone()))]),
        ..Default::default()
    };
    assert_eq!(get(&net, page, 1000).await.unwrap(), other);
    assert_eq!(net.calls.lock().unwrap().len(), 1);
    let net = Net {
        answers: HashMap::from([(
            page.into(),
            HttpReply {
                status: 404,
                ..Default::default()
            },
        )]),
        ..Default::default()
    };
    assert_eq!(get(&net, page, 1000).await.unwrap_err().code, "http");
    assert_eq!(net.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn dns_deadline_followup_budget_and_error_conversion_assertions() {
    let net = Net {
        dns_delay: true,
        ..Default::default()
    };
    assert_eq!(
        fetch::fetch_with(
            "https://public.test/x",
            1000,
            &[],
            Duration::from_millis(10),
            &net
        )
        .await
        .unwrap_err()
        .code,
        "timeout"
    );
    assert!(net.calls.lock().unwrap().is_empty());
    let page = "https://paste.example/raw/x";
    let link = "https://paste.example/api/stream/x";
    let net = Net {
        answers: HashMap::from([
            (page.into(), reply(envelope(link))),
            (link.into(), reply(b"print(1)")),
        ]),
        response_delay: Duration::from_millis(10),
        ..Default::default()
    };
    fetch::fetch_with_agent(
        page,
        1234,
        &["paste.example".into()],
        Duration::from_secs(7),
        &net,
        "me/1",
    )
    .await
    .unwrap();
    {
        let budgets = net.budgets.lock().unwrap();
        assert_eq!(budgets.len(), 2);
        assert_eq!(budgets[0].0, 1234);
        assert_eq!(budgets[1].0, 1234);
        assert!(
            budgets[1].1 > Duration::ZERO
                && budgets[1].1 < budgets[0].1
                && budgets[0].1 <= Duration::from_secs(7)
        );
    }
    for (code, detail) in [
        ("http", "the server answered HTTP 403"),
        ("html", "that is a web page, not a raw file"),
        ("timeout", "the server took too long to respond"),
        ("net", "could not connect to that server"),
        ("blocked", "that address is not a public address"),
        ("bad_url", "invalid URL"),
        ("empty", "the content is empty"),
        ("too_big", "the file exceeds the limit"),
    ] {
        let net = Net {
            answers: HashMap::from([(page.into(), reply(envelope(link)))]),
            errors: HashMap::from([(link.into(), (code, detail))]),
            ..Default::default()
        };
        let error = get(&net, page, 1000).await.unwrap_err();
        assert_eq!(
            error.code,
            if code == "too_big" {
                "too_big"
            } else {
                "loader"
            }
        );
        assert_eq!(error.message, detail);
        assert_eq!(net.calls.lock().unwrap().len(), 2);
    }
}

#[tokio::test]
async fn four_redirect_agent_cases_and_compressed_redirect_content_headers_are_fresh() {
    let ordinary = "https://public.test/x";
    let bacon = format!("https://{}/x", fetch::ROBLOX_PASTE_HOST);
    for (start, end, agent, want) in [
        (
            ordinary,
            bacon.as_str(),
            fetch::USER_AGENT,
            "Roblox/WinInet",
        ),
        (
            bacon.as_str(),
            ordinary,
            fetch::USER_AGENT,
            fetch::USER_AGENT,
        ),
        (
            ordinary,
            bacon.as_str(),
            "custom-client/1",
            "custom-client/1",
        ),
        (
            bacon.as_str(),
            ordinary,
            "custom-client/1",
            "custom-client/1",
        ),
    ] {
        let net = Net {
            answers: HashMap::from([
                (
                    start.into(),
                    HttpReply {
                        encoding: "gzip".into(),
                        content_type: "text/html".into(),
                        ..redirect(end)
                    },
                ),
                (end.into(), reply(b"print(1)")),
            ]),
            ..Default::default()
        };
        assert_eq!(
            fetch::fetch_with_agent(start, 1000, &[], Duration::from_secs(1), &net, agent)
                .await
                .unwrap(),
            b"print(1)"
        );
        assert_eq!(net.calls.lock().unwrap()[1].2, want);
    }
    let mut zip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    zip.write_all(b"print(1)").unwrap();
    let net = Net {
        answers: HashMap::from([
            (ordinary.into(), redirect("/gz")),
            (
                "https://public.test/gz".into(),
                HttpReply {
                    encoding: "gzip".into(),
                    body: zip.finish().unwrap(),
                    status: 200,
                    ..Default::default()
                },
            ),
        ]),
        ..Default::default()
    };
    assert_eq!(get(&net, ordinary, 1000).await.unwrap(), b"print(1)");
}

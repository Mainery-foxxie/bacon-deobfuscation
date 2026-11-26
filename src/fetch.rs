use crate::{
    error::{Error, Result},
    paste_envelope::stream_url,
};
use flate2::read::{GzDecoder, ZlibDecoder};
use regex::Regex;
use serenity::async_trait;
use std::{
    io::Read,
    net::{IpAddr, SocketAddr},
    time::Duration,
};
use url::Url;

pub const USER_AGENT: &str = "bgdeobf-bot/1.0 (script fetch)";
pub const ROBLOX_PASTE_HOST: &str = "paste-web.baconbossscript.workers.dev";
fn error(code: &str, msg: &str) -> Error {
    Error::new(code, msg)
}
fn timeout_error() -> Error {
    error("timeout", "the server took too long to respond")
}

pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => {
            let n = u32::from(a);
            ![
                (0, 8),
                (0x0a000000, 8),
                (0x64400000, 10),
                (0x7f000000, 8),
                (0xa9fe0000, 16),
                (0xac100000, 12),
                (0xc0000000, 24),
                (0xc0000200, 24),
                (0xc0586300, 24),
                (0xc0a80000, 16),
                (0xc6120000, 15),
                (0xc6336400, 24),
                (0xcb007100, 24),
                (0xe0000000, 4),
                (0xf0000000, 4),
            ]
            .iter()
            .any(|&(base, bits)| n >> (32 - bits) == base >> (32 - bits))
        }
        IpAddr::V6(a) => {
            if let Some(v4) = a.to_ipv4_mapped() {
                return is_public(IpAddr::V4(v4));
            }
            let n = u128::from(a);
            let in_net = |base: u128, bits: u32| n >> (128 - bits) == base >> (128 - bits);
            // Preserve fetch.py's explicit blocks plus the reference's is_global exclusions.
            ![
                (0, 128),
                (1, 128),
                (0x0064ff9bu128 << 96, 96),
                (0x0064ff9b0001u128 << 80, 48),
                (0x0100u128 << 112, 64),
                (0x2001u128 << 112, 23),
                (0x20010db8u128 << 96, 32),
                (0x2002u128 << 112, 16),
                (0x3fff0u128 << 108, 20),
                (0xfc00u128 << 112, 7),
                (0xfe80u128 << 112, 10),
                (0xfec0u128 << 112, 10),
                (0xff00u128 << 112, 8),
            ]
            .iter()
            .any(|&(base, bits)| in_net(base, bits))
        }
    }
}

pub fn validate_addresses(addresses: &[SocketAddr]) -> Result<()> {
    if addresses.is_empty() {
        return Err(error("net", "could not resolve that server"));
    }
    if addresses.iter().any(|a| !is_public(a.ip())) {
        return Err(error("blocked", "that address is not a public address"));
    }
    Ok(())
}

pub fn extract_url(text: &str) -> Option<String> {
    Regex::new(r#"(?i)https?://[^\s<>"'`)\]]+"#)
        .unwrap()
        .find(text)
        .map(|m| {
            m.as_str()
                .trim_end_matches(['.', ',', ';', ':', '!', '?'])
                .to_owned()
        })
}

pub fn check_url(text: &str, hosts: &[String]) -> Result<Url> {
    let text = text.trim();
    if text.contains('\\') || text.chars().any(char::is_control) {
        return Err(error("bad_url", "invalid URL"));
    }
    let u = Url::parse(text).map_err(|_| error("bad_url", "invalid URL"))?;
    if !matches!(u.scheme(), "http" | "https") {
        return Err(error("bad_url", "only http:// or https:// is allowed"));
    }
    let authority = text
        .split_once("://")
        .map(|(_, s)| s.split(['/', '?', '#']).next().unwrap_or(""))
        .unwrap_or("");
    if authority.contains('@') || !u.username().is_empty() || u.password().is_some() {
        return Err(error("bad_url", "the URL must not contain user:password"));
    }
    let host = u
        .host_str()
        .unwrap_or("")
        .trim_end_matches('.')
        .trim_matches(['[', ']'])
        .to_ascii_lowercase();
    if host.is_empty() || host.len() > 253 {
        return Err(error("bad_url", "invalid URL"));
    }
    if !matches!(u.port_or_known_default(), Some(80 | 443)) {
        return Err(error("bad_url", "only port 80 or 443 is allowed"));
    }
    if host.parse::<IpAddr>().is_ok_and(|a| !is_public(a)) {
        return Err(error("blocked", "that address is not a public address"));
    }
    if !hosts.is_empty()
        && !hosts
            .iter()
            .any(|h| host == *h || host.ends_with(&format!(".{h}")))
    {
        return Err(error("blocked", "that host is not on the allowed list"));
    }
    Ok(u)
}

pub fn normalize_url(text: &str) -> String {
    let text = text.trim();
    // Do not let a raw-link rewrite erase a forbidden authority or port.
    if check_url(text, &[]).is_err() {
        return text.into();
    }
    let u = Url::parse(text).unwrap();
    let host = u.host_str().unwrap_or("");
    let parts: Vec<_> = u.path().split('/').filter(|s| !s.is_empty()).collect();
    if matches!(host, "github.com" | "www.github.com") && parts.len() >= 5 && parts[2] == "blob" {
        return format!(
            "https://raw.githubusercontent.com/{}/{}/{}/{}",
            parts[0],
            parts[1],
            parts[3],
            parts[4..].join("/")
        );
    }
    if matches!(host, "pastebin.com" | "www.pastebin.com")
        && parts.len() == 1
        && Regex::new(r"^[A-Za-z0-9]{6,12}$")
            .unwrap()
            .is_match(parts[0])
    {
        return format!("https://pastebin.com/raw/{}", parts[0]);
    }
    if host == "gist.github.com"
        && parts.len() == 2
        && Regex::new(r"^[a-fA-F0-9]{8,40}$")
            .unwrap()
            .is_match(parts[1])
    {
        return format!(
            "https://gist.githubusercontent.com/{}/{}/raw",
            parts[0], parts[1]
        );
    }
    text.into()
}

pub fn url_stem(text: &str) -> String {
    let Ok(u) = Url::parse(text) else {
        return "script".into();
    };
    let seg = u
        .path()
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("");
    let stem = Regex::new(r"(?i)\.(lua|luau|txt)$")
        .unwrap()
        .replace(seg, "");
    if stem.is_empty() {
        "script".into()
    } else {
        stem.into_owned()
    }
}

#[derive(Default, Debug)]
pub struct HttpReply {
    pub status: u16,
    pub location: Option<String>,
    pub encoding: String,
    pub content_type: String,
    pub body: Vec<u8>,
}

/// The transport receives the exact validated addresses it must connect to.
#[async_trait]
pub trait Transport: Send + Sync {
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>>;
    async fn get(
        &self,
        url: &Url,
        addresses: &[SocketAddr],
        max: usize,
        timeout: Duration,
        agent: &str,
    ) -> Result<HttpReply>;
}
pub struct ReqwestTransport;
struct BoundResolver {
    host: String,
    addresses: Vec<SocketAddr>,
}
impl reqwest::dns::Resolve for BoundResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let permitted = name
            .as_str()
            .trim_end_matches('.')
            .eq_ignore_ascii_case(self.host.trim_end_matches('.'));
        let addresses = self.addresses.clone();
        Box::pin(async move {
            if permitted {
                Ok(Box::new(addresses.into_iter()) as reqwest::dns::Addrs)
            } else {
                Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "unvalidated DNS name",
                ))
                    as Box<dyn std::error::Error + Send + Sync>)
            }
        })
    }
}
fn req_error(e: reqwest::Error) -> Error {
    if e.is_timeout() {
        timeout_error()
    } else {
        error("net", "could not connect to that server")
    }
}
#[async_trait]
impl Transport for ReqwestTransport {
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>> {
        tokio::net::lookup_host((host, port))
            .await
            .map(|a| a.collect())
            .map_err(|_| error("net", "could not resolve that server"))
    }
    async fn get(
        &self,
        url: &Url,
        addresses: &[SocketAddr],
        max: usize,
        timeout: Duration,
        agent: &str,
    ) -> Result<HttpReply> {
        check_url(url.as_str(), &[])?;
        validate_addresses(addresses)?;
        if addresses
            .iter()
            .any(|a| Some(a.port()) != url.port_or_known_default())
        {
            return Err(error("bad_url", "connection port does not match the URL"));
        }
        request(url, addresses, max, timeout, agent).await
    }
}

// Only the checked Transport entry point calls this in production. Unit tests use loopback
// here to exercise the real connector without weakening the public-address policy.
async fn request(
    url: &Url,
    addresses: &[SocketAddr],
    max: usize,
    timeout: Duration,
    agent: &str,
) -> Result<HttpReply> {
    let host = url
        .host_str()
        .ok_or_else(|| error("bad_url", "invalid URL"))?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .no_gzip()
        .no_deflate()
        .no_brotli()
        .no_zstd()
        .redirect(reqwest::redirect::Policy::none())
        .referer(false)
        .pool_max_idle_per_host(0)
        .connect_timeout(Duration::from_secs(10).min(timeout))
        .read_timeout(Duration::from_secs(15).min(timeout))
        .timeout(timeout)
        .dns_resolver(std::sync::Arc::new(BoundResolver {
            host: host.into(),
            addresses: addresses.to_vec(),
        }))
        .build()
        .map_err(req_error)?;
    let mut response = client
        .get(url.clone())
        .header("User-Agent", agent)
        .header("Accept", "text/plain, */*;q=0.5")
        .header("Accept-Encoding", "gzip, identity")
        .header("Cache-Control", "no-cache")
        .send()
        .await
        .map_err(req_error)?;
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned()
    };
    let mut reply = HttpReply {
        status: response.status().as_u16(),
        location: response
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned),
        encoding: header("content-encoding"),
        content_type: header("content-type"),
        body: Vec::new(),
    };
    if reply.status != 200 {
        return Ok(reply);
    }
    if response.content_length().is_some_and(|n| n > max as u64) {
        return Err(error("too_big", "the file exceeds the limit"));
    }
    while let Some(chunk) = response.chunk().await.map_err(req_error)? {
        if chunk.len() > max.saturating_sub(reply.body.len()) {
            return Err(error("too_big", "the file exceeds the limit"));
        }
        reply.body.extend_from_slice(&chunk);
    }
    Ok(reply)
}

pub fn decode_body(body: &[u8], encoding: &str, content_type: &str, max: usize) -> Result<Vec<u8>> {
    if body.len() > max {
        return Err(error("too_big", "the file exceeds the limit"));
    }
    let mut out = Vec::new();
    let reader: Box<dyn Read + '_> = match encoding.trim().to_ascii_lowercase().as_str() {
        "gzip" | "x-gzip" => Box::new(GzDecoder::new(body)),
        "deflate" => Box::new(ZlibDecoder::new(body)),
        "" | "identity" => Box::new(body),
        _ => return Err(error("http", "the server used an unsupported compression")),
    };
    reader
        .take(max.saturating_add(1) as u64)
        .read_to_end(&mut out)
        .map_err(|_| error("http", "the compressed content is corrupt"))?;
    if out.len() > max {
        return Err(error(
            "too_big",
            "the decompressed content exceeds the limit",
        ));
    }
    if crate::astutil::is_blank_source(&out) {
        return Err(error("empty", "the content is empty"));
    }
    let head = String::from_utf8_lossy(&out[..out.len().min(300)])
        .trim_start()
        .to_ascii_lowercase();
    if head.starts_with('<')
        && (content_type.to_ascii_lowercase().contains("html")
            || head.starts_with("<!doctype html")
            || head.starts_with("<html"))
    {
        return Err(error("html", "that is a web page, not a raw file"));
    }
    Ok(out)
}

async fn one(
    url: &str,
    max: usize,
    hosts: &[String],
    deadline: tokio::time::Instant,
    transport: &dyn Transport,
    user_agent: &str,
) -> Result<Vec<u8>> {
    let mut current = url.to_owned();
    for _ in 0..=3 {
        let u = check_url(&current, hosts)?;
        let host = u.host_str().unwrap().trim_matches(['[', ']']);
        let port = u.port_or_known_default().unwrap();
        let addresses = if let Ok(ip) = host.parse::<IpAddr>() {
            vec![SocketAddr::new(ip, port)]
        } else {
            transport.resolve(host, port).await?
        };
        validate_addresses(&addresses)?;
        let budget = deadline.saturating_duration_since(tokio::time::Instant::now());
        if budget.is_zero() {
            return Err(timeout_error());
        }
        let agent = if user_agent == USER_AGENT
            && host
                .trim_end_matches('.')
                .eq_ignore_ascii_case(ROBLOX_PASTE_HOST)
        {
            "Roblox/WinInet"
        } else {
            user_agent
        };
        let response = transport.get(&u, &addresses, max, budget, agent).await?;
        if matches!(response.status, 301 | 302 | 303 | 307 | 308) {
            let location = response
                .location
                .ok_or_else(|| error("http", "redirect without a destination"))?;
            if location.contains('\\') || location.chars().any(char::is_control) {
                return Err(error("bad_url", "invalid URL"));
            }
            if location.contains("://") {
                check_url(&location, hosts)?;
            } else if location.starts_with("//") {
                check_url(&format!("{}:{location}", u.scheme()), hosts)?;
            }
            current = u
                .join(&location)
                .map_err(|_| error("bad_url", "invalid URL"))?
                .into();
            continue;
        }
        if response.status != 200 {
            return Err(error(
                "http",
                &format!("the server answered HTTP {}", response.status),
            ));
        }
        return decode_body(
            &response.body,
            &response.encoding,
            &response.content_type,
            max,
        );
    }
    Err(error("http", "too many redirects"))
}

pub async fn fetch_with(
    url: &str,
    max_bytes: usize,
    hosts: &[String],
    timeout: Duration,
    transport: &dyn Transport,
) -> Result<Vec<u8>> {
    fetch_with_agent(url, max_bytes, hosts, timeout, transport, USER_AGENT).await
}

pub async fn fetch_with_agent(
    url: &str,
    max_bytes: usize,
    hosts: &[String],
    timeout: Duration,
    transport: &dyn Transport,
    user_agent: &str,
) -> Result<Vec<u8>> {
    let deadline = tokio::time::Instant::now() + timeout;
    let url = normalize_url(url);
    let data = tokio::time::timeout_at(
        deadline,
        one(&url, max_bytes, hosts, deadline, transport, user_agent),
    )
    .await
    .map_err(|_| timeout_error())??;
    let Some(link) = stream_url(&data, &url)? else {
        return Ok(data);
    };
    let real = tokio::time::timeout_at(
        deadline,
        one(&link, max_bytes, hosts, deadline, transport, user_agent),
    )
    .await
    .map_err(|_| timeout_error())
    .and_then(|r| r)
    .map_err(|e| {
        if matches!(e.code.as_str(), "too_big" | "loader") {
            e
        } else {
            Error::new("loader", e.message)
        }
    })?;
    if stream_url(&real, &link)?.is_some() {
        return Err(error("loader", "it answered with a loader again"));
    }
    Ok(real)
}

pub async fn fetch(
    url: &str,
    max_bytes: usize,
    hosts: &[String],
    timeout: Duration,
) -> Result<Vec<u8>> {
    fetch_with(url, max_bytes, hosts, timeout, &ReqwestTransport).await
}

#[cfg(test)]
mod connection_tests {
    use super::*;
    use reqwest::dns::Resolve;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn serve(raw: Vec<u8>, max: usize) -> (Result<HttpReply>, String) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buf = [0; 2048];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = stream.read(&mut buf).await.unwrap();
                assert!(n > 0);
                request.extend_from_slice(&buf[..n]);
            }
            let _ = stream.write_all(&raw).await;
            String::from_utf8(request).unwrap()
        });
        let url = Url::parse(&format!(
            "http://public.invalid:{}/script.lua",
            address.port()
        ))
        .unwrap();
        let response = request(&url, &[address], max, Duration::from_secs(3), USER_AGENT).await;
        (response, task.await.unwrap())
    }

    #[tokio::test]
    async fn real_reqwest_connects_to_pinned_address_and_preserves_host_headers() {
        let(raw,headers)=serve(b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nContent-Type: text/plain\r\nSet-Cookie: secret=x\r\nConnection: close\r\n\r\nprint(1)".to_vec(),1000).await;
        assert_eq!(raw.unwrap().body, b"print(1)");
        let headers = headers.to_ascii_lowercase();
        assert!(headers.contains("host: public.invalid:"));
        assert!(headers.contains("cache-control: no-cache"));
        assert!(headers.contains("accept-encoding: gzip, identity"));
        assert!(headers.contains("user-agent: bgdeobf-bot/1.0 (script fetch)"));
        assert!(!headers.contains("cookie:"));
    }

    #[tokio::test]
    async fn real_reqwest_does_not_follow_redirects_or_auto_decompress() {
        let(reply,_)=serve(b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),1000).await;
        assert_eq!(reply.unwrap().status, 302);
        let body = vec![0x1f, 0x8b, 8, 0];
        let mut raw=b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 4\r\nConnection: close\r\n\r\n".to_vec();
        raw.extend_from_slice(&body);
        let (reply, _) = serve(raw, 1000).await;
        assert_eq!(reply.unwrap().body, body);
    }

    #[tokio::test]
    async fn real_reqwest_caps_content_length_and_chunked_bodies() {
        let (reply, _) = serve(
            b"HTTP/1.1 200 OK\r\nContent-Length: 1000000\r\nConnection: close\r\n\r\n".to_vec(),
            10,
        )
        .await;
        assert_eq!(reply.unwrap_err().code, "too_big");
        let(reply,_)=serve(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n6\r\n123456\r\n6\r\n789012\r\n0\r\n\r\n".to_vec(),10).await;
        assert_eq!(reply.unwrap_err().code, "too_big");
    }

    #[tokio::test]
    async fn connection_resolver_has_no_system_dns_fallback() {
        let expected = vec![
            "8.8.8.8:443".parse().unwrap(),
            "[2606:4700:4700::1111]:443".parse().unwrap(),
        ];
        let resolver = BoundResolver {
            host: "public.invalid".into(),
            addresses: expected.clone(),
        };
        assert_eq!(
            resolver
                .resolve("PUBLIC.invalid".parse().unwrap())
                .await
                .unwrap()
                .collect::<Vec<_>>(),
            expected
        );
        assert!(resolver
            .resolve("unvalidated.invalid".parse().unwrap())
            .await
            .is_err());
        assert_eq!(
            ReqwestTransport
                .get(
                    &Url::parse("http://127.0.0.1/").unwrap(),
                    &expected,
                    1000,
                    Duration::from_millis(1),
                    USER_AGENT
                )
                .await
                .unwrap_err()
                .code,
            "blocked"
        );
    }
}

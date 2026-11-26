use bgdeobf::{bot, card::Card, messages, namesfile};
use bgdeobf::{
    error::{Error, Result},
    model::{Options, Output},
};
use bot::{Backend, Delivery, Handler, Identity, InputFile, Network, Progress, Request, Responder};
use serde_json::json;
use serenity::async_trait;
use std::collections::BTreeMap;
use std::{
    io::Read,
    sync::{Arc, Mutex},
    time::Duration,
};

#[test]
fn config_dotenv_prefix_and_names_regressions() {
    let mut env = BTreeMap::from([("WORKERS".into(), "9".into())]);
    bot::parse_dotenv(
        "# c\nDISCORD_TOKEN=\"tok en\"\nexport MAX_MB=2\nBAD LINE\nWORKERS=3",
        &mut env,
    );
    assert_eq!(env["WORKERS"], "9");
    assert_eq!(env["DISCORD_TOKEN"], "tok en");
    let cfg = bot::Config::from_env(&env).unwrap();
    assert_eq!(cfg.workers, 9);
    assert_eq!(cfg.max_bytes(), 2 * 1024 * 1024);
    let cfg = bot::Config::from_env(&BTreeMap::from([
        ("GUILD_ID".into(), " 900 ".into()),
        ("ALLOWED_USERS".into(), "1, 2 3".into()),
        ("MAX_MB".into(), " 2 ".into()),
        ("WORKERS".into(), " 4 ".into()),
        ("MAX_QUEUE".into(), " 7 ".into()),
        ("JOB_TIMEOUT".into(), " 240 ".into()),
        ("COOLDOWN".into(), " 1.5 ".into()),
    ]))
    .unwrap();
    assert_eq!(cfg.guild_id, Some(900));
    assert_eq!(cfg.allowed_users, [1, 2, 3].into());
    assert_eq!((cfg.max_mb, cfg.workers, cfg.max_queue), (2.0, 4, 7));
    assert_eq!(cfg.timeout, Duration::from_secs(240));
    assert_eq!(cfg.cooldown, Duration::from_secs_f64(1.5));
    for (k, v) in [
        ("MAX_MB", "0"),
        ("MAX_MB", "NaN"),
        ("WORKERS", "0"),
        ("JOB_TIMEOUT", "5"),
        ("GUILD_ID", "x"),
        ("ALLOWED_USERS", "1, nope"),
        ("PREFIX", "abcd"),
        ("PREFIX", "a b"),
        ("URL_HOSTS", "ex ample!.com"),
        ("COOLDOWN", "-1"),
    ] {
        assert!(
            bot::Config::from_env(&BTreeMap::from([(k.into(), v.into())])).is_err(),
            "{k}={v}"
        );
    }
    assert_eq!(
        bot::parse_command("  !UWU trace x", "!", "uwu"),
        Some("trace x".into())
    );
    assert_eq!(bot::parse_command("!uwux", "!", "uwu"), None);
    assert_eq!(bot::safe_stem("../../etc/Pass wd.lua"), "Pass_wd");
    assert_eq!(bot::safe_stem("..."), "script");
    assert_eq!(bot::safe_stem(".lua"), "lua");
    assert_eq!(
        namesfile::parse_names(b"{\"12:34\":\"playerList\",\"12:p1\":\"target\"}").unwrap()
            ["12:p1"],
        "target"
    );
    for bad in [
        b"[]".as_slice(),
        b"{\"oops\":\"x\"}",
        b"{\"12:34\":\"1bad\"}",
        b"\xff",
    ] {
        assert!(namesfile::parse_names(bad).is_err());
    }
}

#[test]
fn verification_requires_complete_delivery_evidence() {
    let mut info = json!({"verification":{"raw_syntax":"passed","delivered_syntax":"passed","raw_bytecode":"passed","raw_applies_to_delivery":true},"fidelity":{"functions":3,"functions_recompiled":3,"identical":1,"reordered":1,"cosmetic":1,"differing":[]}});
    assert_eq!(
        messages::verification_status(&info).as_deref(),
        Some("BYTECODE CHECK PASSED")
    );
    assert!(messages::verification_report(&info).contains("Real Roblox / executor: not tested"));
    info["verification"]["raw_applies_to_delivery"] = json!(false);
    assert_eq!(
        messages::verification_status(&info).as_deref(),
        Some("PARTIAL")
    );
    info["verification"]["delivered_syntax"] = json!("failed");
    assert_eq!(
        messages::verification_status(&info).as_deref(),
        Some("FAILED")
    );
    info["trace_only"] = json!(true);
    assert_eq!(messages::verification_status(&info), None);
    assert!(!messages::explain("internal", "/srv/secret/path").contains("secret"));
    assert_eq!(
        messages::scrub("C:\\Users\\Someone\\AppData\\tmp1.lua(8,14): SyntaxError"),
        "line 8:14: SyntaxError"
    );
}

#[test]
fn full_verification_evidence_scope_and_source_free_report_matrix() {
    let base = json!({"verification":{"raw_syntax":"passed","delivered_syntax":"passed","raw_bytecode":"passed","raw_applies_to_delivery":true,"fake_trace":"not run"},"fidelity":{"functions":3,"functions_recompiled":3,"identical":1,"reordered":1,"cosmetic":1,"differing":[]}});
    for (changes, status) in [
        (json!({}), "BYTECODE CHECK PASSED"),
        (json!({"delivered_syntax":"failed"}), "FAILED"),
        (json!({"raw_applies_to_delivery":false}), "PARTIAL"),
        (json!({"raw_bytecode":"differences found"}), "PARTIAL"),
        (json!({"raw_bytecode":"incomplete"}), "PARTIAL"),
        (
            json!({"raw_syntax":"unavailable","delivered_syntax":"unavailable"}),
            "PARTIAL",
        ),
        (
            json!({"raw_syntax":"not run","delivered_syntax":"not run","raw_bytecode":"not run"}),
            "PARTIAL",
        ),
        (
            json!({"delivered_syntax":"not runnable","raw_applies_to_delivery":false}),
            "PARTIAL",
        ),
    ] {
        let mut info = base.clone();
        for (k, v) in changes.as_object().unwrap() {
            info["verification"][k] = v.clone();
        }
        assert_eq!(
            messages::verification_status(&info).as_deref(),
            Some(status)
        );
        let report = messages::verification_report(&info);
        assert!(report.starts_with(&format!("Status: {status}\n")));
        assert!(report.contains("Real Roblox / executor: not tested"));
        assert!(report.contains("not a guarantee of semantic equivalence"));
    }
    for flags in [
        json!({"trace_only":true}),
        json!({"sandbox_only":true}),
        json!({"kind":"moonveil"}),
    ] {
        let mut info = base.clone();
        for (k, v) in flags.as_object().unwrap() {
            info[k] = v.clone();
        }
        assert!(messages::verification_status(&info).is_none());
        assert_eq!(messages::verification_report(&info), "");
    }
    for changes in [
        json!({"functions":0,"functions_recompiled":0,"identical":0,"reordered":0,"cosmetic":0}),
        json!({"functions_recompiled":4}),
        json!({"identical":0}),
        json!({"differing":[1]}),
    ] {
        let mut info = base.clone();
        for (k, v) in changes.as_object().unwrap() {
            info["fidelity"][k] = v.clone();
        }
        assert_eq!(
            messages::verification_status(&info).as_deref(),
            Some("PARTIAL")
        );
    }
    for evidence in [
        serde_json::Value::Null,
        json!({}),
        json!({"raw_applies_to_delivery":null}),
    ] {
        let mut info = base.clone();
        info["verification"] = evidence;
        let report = messages::verification_report(&info);
        assert!(report.contains("Delivered Lua syntax: not run"));
        assert!(report.contains("Scope: unknown; delivery coverage was not recorded."));
        assert!(!report.contains("cleaned file"));
        assert_eq!(
            messages::verification_status(&info).as_deref(),
            Some("PARTIAL")
        );
    }
    let report = messages::verification_report(&base);
    assert!(report.contains("3 decoded, 3 recompiled; 3 accepted, 0 differ"));
    assert!(report.contains("1 identical, 1 reordered, 1 cosmetic"));
    assert!(report.contains("Scope: raw checks apply to the delivered Lua, where available."));
    let mut cleaned = base.clone();
    cleaned["cleaned"] = json!({"statements":1});
    cleaned["verification"]["raw_applies_to_delivery"] = json!(false);
    let report = messages::verification_report(&cleaned);
    assert!(report.contains("Raw Lua syntax: passed"));
    assert!(report.contains("Delivered Lua syntax: passed"));
    assert!(report.contains("does not cover the cleaned file"));
    cleaned["annotated"] = json!(true);
    cleaned["verification"]["delivered_syntax"] = json!("not runnable");
    let report = messages::verification_report(&cleaned);
    assert!(report.contains("Raw Lua syntax (unannotated copy): passed"));
    assert!(report.contains("Delivered Lua syntax: not runnable"));
    for (trace, needle) in [
        ("not run", "Fake Roblox trace: not run"),
        ("unavailable", "Fake Roblox trace: unavailable"),
        ("observed", "Fake Roblox trace: observed (simulated only"),
    ] {
        let mut info = base.clone();
        info["verification"]["fake_trace"] = json!(trace);
        info["verification"]["raw_bytecode"] = json!("differences found");
        assert!(messages::verification_report(&info).contains(needle));
        assert_eq!(
            messages::verification_status(&info).as_deref(),
            Some("PARTIAL")
        );
    }
    let mut hostile = base;
    hostile["warnings"] = json!(["C:/Users/private/payload.lua"]);
    hostile["trace"] = json!({"reason":"@everyone secret source","findings":["secret-source"]});
    hostile["source"] = json!("secret-source");
    let report = messages::verification_report(&hostile);
    for text in ["private", "@everyone", "secret-source", "payload.lua"] {
        assert!(!report.contains(text));
    }
    assert!(report.len() < 2000);
}

#[test]
fn moonveil_command_parsing_help_and_honest_verdict() {
    for (text, expected) in [
        ("!moonveil", Some("")),
        ("!moonveil   ", Some("")),
        ("!MoonVeil trace", Some("trace")),
        ("  !moonveil https://a.b/c.lua", Some("https://a.b/c.lua")),
        ("!moonveil\ntrace", Some("trace")),
        ("!moonveilx", None),
        ("!moonveils", None),
        ("halo !moonveil", None),
        ("!uwu", None),
        ("moonveil", None),
        ("", None),
    ] {
        assert_eq!(
            bot::parse_command(text, "!", "moonveil").as_deref(),
            expected
        );
    }
    assert_eq!(bot::parse_command("!moonveil", "!", "uwu"), None);
    assert_eq!(
        bot::parse_command("?moonveil trace", "?", "moonveil").as_deref(),
        Some("trace")
    );
    let h = make(
        config(),
        Arc::new(FakeBackend::default()),
        Arc::new(FakeNetwork::default()),
    );
    assert!(h.info_text().chars().count() < 2000);
    assert!(h.info_text().contains("`!moonveil`"));
    assert!(h.chat_help(false).contains("`!moonveil`"));
    let help = h.chat_help(true);
    for needle in ["`!moonveil`", "_reconstructed.lua", "trace", "`!uwu`"] {
        assert!(help.contains(needle));
    }
    let mut info = json!({"kind":"moonveil","trace_only":true,"moonveil":{"version":"2.1.1"},"lines":40,"calls":120,"threads":5,"downloads":["https://example.com/a.lua"],"warnings":[],"syntax_ok":true});
    let lines = messages::verdict(&info);
    assert_eq!(lines[0], "Detected: MoonVeil 2.1.1 (private VM protector)");
    let text = lines.join("\n");
    for needle in [
        "NOT the original source",
        "120 calls seen in 5 threads/callbacks, 40 output lines",
        "https://example.com/a.lua",
        "not fetched",
        "Output syntax is valid",
    ] {
        assert!(text.contains(needle));
    }
    assert!(!text.contains("Bytecode"));
    assert!(!text.contains("functions,"));
    info["moonveil"] = json!({"version":"?","assumed":true});
    let lines = messages::verdict(&info);
    assert!(lines[0].starts_with("No MoonVeil banner found"));
    assert!(!lines.join("\n").contains("Detected: MoonVeil"));
}

#[tokio::test]
async fn moonveil_chat_url_reply_and_trace_pass_all_flags() {
    for (args, reference) in [
        ("trace https://x.io/hub.lua", None),
        ("trace", Some("https://x.io/hub.lua")),
    ] {
        let backend = Arc::new(FakeBackend::default());
        let h = make(config(), backend.clone(), Arc::new(FakeNetwork::default()));
        let q = bot::chat_request(args, &[], reference.map(|s| ([].as_slice(), s)), true);
        let mut r = Recorder::default();
        h.run(&mut r, q).await.unwrap();
        let (d, _) = r.final_.unwrap();
        assert_eq!(
            d.files.iter().map(|f| f.0.as_str()).collect::<Vec<_>>(),
            vec!["hub_reconstructed.lua", "hub_trace.txt", "hub_payloads.zip"]
        );
        let seen = backend.seen.lock().unwrap();
        assert!(seen[0].1.moonveil && seen[0].1.trace && !seen[0].1.sandbox_only);
    }
}

#[test]
fn verification_report_preserves_every_evidence_and_scope_distinction() {
    let base = json!({"protos":3,"verification":{"raw_syntax":"passed","delivered_syntax":"passed","raw_bytecode":"passed","raw_applies_to_delivery":true,"fake_trace":"not run"},"fidelity":{"functions":3,"functions_recompiled":3,"identical":1,"reordered":1,"cosmetic":1,"differing":[]}});
    for (changes, status) in [
        (json!({}), "BYTECODE CHECK PASSED"),
        (json!({"delivered_syntax":"failed"}), "FAILED"),
        (json!({"raw_applies_to_delivery":false}), "PARTIAL"),
        (json!({"raw_bytecode":"differences found"}), "PARTIAL"),
        (json!({"raw_bytecode":"incomplete"}), "PARTIAL"),
        (
            json!({"raw_syntax":"unavailable","delivered_syntax":"unavailable"}),
            "PARTIAL",
        ),
        (
            json!({"raw_syntax":"not run","delivered_syntax":"not run","raw_bytecode":"not run"}),
            "PARTIAL",
        ),
        (
            json!({"delivered_syntax":"not runnable","raw_applies_to_delivery":false}),
            "PARTIAL",
        ),
    ] {
        let mut i = base.clone();
        i["verification"]
            .as_object_mut()
            .unwrap()
            .extend(changes.as_object().unwrap().clone());
        assert_eq!(messages::verification_status(&i).as_deref(), Some(status));
        let report = messages::verification_report(&i);
        assert!(report.starts_with(&format!("Status: {status}\n")));
        assert!(report.contains("Real Roblox / executor: not tested"));
        assert!(report.contains("not a guarantee of semantic equivalence"));
    }
    for changes in [
        json!({"functions":0,"functions_recompiled":0,"identical":0,"reordered":0,"cosmetic":0}),
        json!({"functions_recompiled":4}),
        json!({"identical":0}),
        json!({"differing":[1]}),
    ] {
        let mut i = base.clone();
        i["fidelity"]
            .as_object_mut()
            .unwrap()
            .extend(changes.as_object().unwrap().clone());
        assert_eq!(
            messages::verification_status(&i).as_deref(),
            Some("PARTIAL")
        );
    }
    for flags in [
        json!({"trace_only":true}),
        json!({"sandbox_only":true}),
        json!({"kind":"moonveil"}),
    ] {
        let mut i = base.clone();
        i.as_object_mut()
            .unwrap()
            .extend(flags.as_object().unwrap().clone());
        assert_eq!(messages::verification_status(&i), None);
        assert_eq!(messages::verification_report(&i), "");
    }
    for verification in [
        None,
        Some(json!({})),
        Some(json!({"raw_applies_to_delivery":null})),
    ] {
        let mut i = json!({"syntax_ok":true,"fidelity":{"functions":3,"identical":3,"reordered":0,"cosmetic":0,"differing":[]}});
        if let Some(v) = verification {
            i["verification"] = v;
        }
        assert_eq!(
            messages::verification_status(&i).as_deref(),
            Some("PARTIAL")
        );
        let report = messages::verification_report(&i);
        assert!(report.contains("Delivered Lua syntax: not run"));
        assert!(report.contains("Scope: unknown; delivery coverage was not recorded."));
        assert!(!report.contains("cleaned file"));
    }
    let report = messages::verification_report(&base);
    for text in [
        "3 decoded, 3 recompiled; 3 accepted, 0 differ",
        "1 identical, 1 reordered, 1 cosmetic",
        "Scope: raw checks apply to the delivered Lua, where available.",
    ] {
        assert!(report.contains(text), "{report}");
    }
    let mut i = base.clone();
    i["verification"]["raw_applies_to_delivery"] = json!(false);
    i["cleaned"] = json!({"statements":1});
    let report = messages::verification_report(&i);
    assert!(report.contains("Raw Lua syntax: passed"));
    assert!(report.contains("Delivered Lua syntax: passed"));
    assert!(report.contains("does not cover the cleaned file"));
    i["annotated"] = json!(true);
    i["verification"]["delivered_syntax"] = json!("not runnable");
    let report = messages::verification_report(&i);
    assert!(report.contains("Raw Lua syntax (unannotated copy): passed"));
    assert!(report.contains("Delivered Lua syntax: not runnable"));
    for (trace, needle) in [
        ("not run", "Fake Roblox trace: not run"),
        ("unavailable", "Fake Roblox trace: unavailable"),
        ("observed", "Fake Roblox trace: observed (simulated only"),
    ] {
        let mut i = base.clone();
        i["verification"]["raw_bytecode"] = json!("differences found");
        i["verification"]["fake_trace"] = json!(trace);
        i["trace"] = json!({"finished":false,"errors":1,"reason":"@everyone secret source","findings":["secret-source"]});
        i["warnings"] = json!(["C:/Users/private/payload.lua"]);
        i["source"] = json!("secret-source");
        let report = messages::verification_report(&i);
        assert!(report.contains(needle));
        assert_eq!(
            messages::verification_status(&i).as_deref(),
            Some("PARTIAL")
        );
        assert!(report.len() < 2000);
        for bad in ["private", "@everyone", "secret-source", "payload.lua"] {
            assert!(!report.contains(bad));
        }
    }
}

#[tokio::test]
async fn startup_validates_before_connecting_and_prefix_opt_out_removes_message_intents() {
    for (env, tools, needle) in [
        (BTreeMap::new(), true, "DISCORD_TOKEN is not set"),
        (
            BTreeMap::from([
                ("DISCORD_TOKEN".into(), "x".into()),
                ("MAX_MB".into(), "99".into()),
            ]),
            true,
            "Invalid configuration",
        ),
        (
            BTreeMap::from([("DISCORD_TOKEN".into(), "x".into())]),
            false,
            "Problem with a Luau tool",
        ),
    ] {
        let error = bot::start_with(
            &env,
            || vec![("luau".into(), tools, "not found".into())],
            |_, _| async { panic!("invalid startup reached Discord") },
        )
        .await
        .unwrap_err();
        assert!(error.message.contains(needle), "{}", error.message);
    }
    for enabled in [true, false] {
        let env = BTreeMap::from([
            ("DISCORD_TOKEN".into(), " x.y.z ".into()),
            (
                "PREFIX_COMMANDS".into(),
                if enabled { "1" } else { "0" }.into(),
            ),
        ]);
        bot::start_with(
            &env,
            || vec![("luau".into(), true, "ok".into())],
            |cfg, intents| async move {
                assert_eq!(cfg.token, "x.y.z");
                assert_eq!(
                    intents.contains(serenity::all::GatewayIntents::MESSAGE_CONTENT),
                    enabled
                );
                assert_eq!(
                    intents.contains(serenity::all::GatewayIntents::GUILD_MESSAGES),
                    enabled
                );
                assert_eq!(
                    intents.contains(serenity::all::GatewayIntents::DIRECT_MESSAGES),
                    enabled
                );
                assert!(intents.contains(serenity::all::GatewayIntents::GUILDS));
                Ok(())
            },
        )
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn startup_explains_real_serenity_login_intent_network_and_http_errors() {
    use serenity::{all::GatewayError, Error as DiscordError};
    for (error, needle) in [
        (
            DiscordError::Gateway(GatewayError::InvalidAuthentication),
            "Discord rejected the token",
        ),
        (
            DiscordError::Gateway(GatewayError::DisallowedGatewayIntents),
            "MESSAGE CONTENT INTENT",
        ),
        (
            DiscordError::Io(std::io::Error::other("no route /srv/secret")),
            "Couldn't connect to Discord",
        ),
    ] {
        let text = bot::startup_error(error, "?").message;
        assert!(text.contains(needle), "{text}");
        assert!(!text.contains("secret"));
        if needle == "MESSAGE CONTENT INTENT" {
            assert!(text.contains("PREFIX_COMMANDS=0") && text.contains("?uwu"));
        }
    }
    let (http, _, task) = discord_server(vec![(
        400,
        "{\"code\":50035,\"message\":\"bad request\"}".into(),
    )])
    .await;
    let error = serenity::all::ChannelId::new(777)
        .say(http, "x")
        .await
        .unwrap_err();
    task.await.unwrap();
    let text = bot::startup_error(error, "!").message;
    assert!(
        text.contains("Discord rejected the request (HTTP 400)"),
        "{text}"
    );
}

#[test]
fn card_terminal_states_and_engine_stages() {
    let mut c = Card::new(true, None);
    c.begin("download");
    assert!(c.data().description.contains("**⠋ Downloading content**"));
    c.queued(2);
    assert!(c.data().description.contains("position 2"));
    c.stage("verify");
    assert!(c.data().description.contains("verifying the result"));
    assert!(!c.data().description.contains("queue"));
    c.complete();
    let d = c.data();
    c.fail();
    assert_eq!(c.data(), d);
}

fn info() -> serde_json::Value {
    json!({"banner":{"version":"0.42.0","protection":"FORGE Protection","build":"abc123"},"protos":3,"records":10,"lines":1,"warnings":[],"fidelity":{"functions":3,"functions_recompiled":3,"identical":3,"reordered":0,"cosmetic":0,"differing":[]},"verification":{"raw_syntax":"passed","delivered_syntax":"passed","raw_bytecode":"passed","raw_applies_to_delivery":true,"fake_trace":"not run"}})
}
fn output() -> Output {
    Output {
        text: Some("print(\"hi\")\n".into()),
        info: info(),
        trace: None,
        raw: None,
    }
}
fn config() -> bot::Config {
    bot::Config::from_env(&BTreeMap::from([("COOLDOWN".into(), "0".into())])).unwrap()
}
fn file(name: &str) -> InputFile {
    InputFile {
        filename: name.into(),
        size: 100,
        url: "https://cdn.discordapp.com/script.lua".into(),
    }
}
#[derive(Default)]
struct Recorder {
    user: u64,
    guild: Option<u64>,
    accepted: bool,
    texts: Vec<String>,
    cards: Vec<bgdeobf::card::CardData>,
    final_: Option<(Delivery, Option<bgdeobf::card::CardData>)>,
}
#[async_trait]
impl Responder for Recorder {
    fn identity(&self) -> Identity {
        Identity {
            user: self.user,
            guild: self.guild,
            upload_limit: bot::DM_UPLOAD_LIMIT,
        }
    }
    async fn accept(&mut self) -> Result<()> {
        self.accepted = true;
        Ok(())
    }
    async fn status(&mut self, text: &str) -> Result<()> {
        self.texts.push(text.into());
        Ok(())
    }
    async fn show(&mut self, card: bgdeobf::card::CardData) -> Result<()> {
        self.cards.push(card);
        Ok(())
    }
    async fn finish(&mut self, d: Delivery, c: Option<bgdeobf::card::CardData>) -> Result<()> {
        self.final_ = Some((d, c));
        Ok(())
    }
}
type DownloadCall = (String, usize, Vec<String>, Duration);
struct FakeNetwork {
    data: Vec<u8>,
    names: Vec<u8>,
    error: Option<&'static str>,
    names_error: Option<&'static str>,
    calls: Mutex<Vec<DownloadCall>>,
}
impl Default for FakeNetwork {
    fn default() -> Self {
        Self {
            data: b"-- protected".to_vec(),
            names: b"{\"12:34\":\"playerList\"}".to_vec(),
            error: None,
            names_error: None,
            calls: Mutex::new(Vec::new()),
        }
    }
}
#[async_trait]
impl Network for FakeNetwork {
    async fn download(
        &self,
        url: &str,
        max: usize,
        hosts: &[String],
        timeout: Duration,
    ) -> Result<Vec<u8>> {
        self.calls
            .lock()
            .unwrap()
            .push((url.into(), max, hosts.to_vec(), timeout));
        let error = if url.ends_with("names.json") {
            self.names_error.or(self.error)
        } else {
            self.error
        };
        if let Some(code) = error {
            return Err(Error::new(code, "HTTP 403"));
        }
        Ok(if url.ends_with("names.json") {
            self.names.clone()
        } else {
            self.data.clone()
        })
    }
}
#[derive(Default)]
struct FakeBackend {
    error: Option<&'static str>,
    error_detail: Option<&'static str>,
    result: Option<Output>,
    panic: bool,
    silent: bool,
    delay: Duration,
    seen: Mutex<Vec<(Vec<u8>, Options)>>,
}
#[async_trait]
impl Backend for FakeBackend {
    async fn submit(
        &self,
        _: u64,
        data: Vec<u8>,
        opts: Options,
        progress: Option<Progress>,
    ) -> Result<Output> {
        assert!(!self.panic, "backend crashed /srv/secret");
        self.seen.lock().unwrap().push((data, opts.clone()));
        if let Some(p) = progress.filter(|_| !self.silent) {
            p("queued:1");
            tokio::time::sleep(self.delay).await;
            for stage in [
                "analyse",
                "unpack",
                "classify",
                "decompile",
                "verify",
                "sandbox",
            ] {
                p(stage);
                tokio::time::sleep(self.delay).await;
            }
        }
        if let Some(code) = self.error {
            return Err(Error::new(
                code,
                self.error_detail
                    .unwrap_or("unknown operation 77 /srv/secret/path"),
            ));
        }
        if let Some(result) = &self.result {
            return Ok(result.clone());
        }
        let mut result = output();
        if opts.annotate {
            result.info["annotated"] = json!(true);
        }
        if opts.moonveil {
            result.info = json!({"kind":"moonveil","trace_only":true,"moonveil":{"version":"2.1.1"},"calls":120,"threads":5,"lines":40});
        }
        if opts.trace || opts.sandbox_only {
            let summary = json!({"events":40,"requests":2,"domains":["api.example.com","cdn.example.org"],"payloads":1,"remotes":1,"instances":12,"errors":0,"findings":["Downloads code and runs it"],"reason":"done","finished":true});
            result.trace = Some(
                json!({"report":"FAKE ROBLOX SANDBOX TRACE  -  @@NAME@@\nsaved as @@STEM@@_payload1.lua\n","payloads":[{"id":1,"ext":"lua","data":[112,114,105,110,116,40,49,41]}],"summary":summary}),
            );
            result.info["trace"] = summary;
            if opts.sandbox_only {
                result.text = None;
                result.info["sandbox_only"] = json!(true);
            }
        }
        Ok(result)
    }
}
fn make(cfg: bot::Config, backend: Arc<FakeBackend>, network: Arc<FakeNetwork>) -> Handler {
    Handler::new(cfg, backend, network)
}

#[tokio::test]
async fn file_url_names_trace_sandbox_and_moonveil_delivery() {
    for mode in ["file", "url", "names", "trace", "sandbox", "moonveil"] {
        let backend = Arc::new(FakeBackend::default());
        let net = Arc::new(FakeNetwork::default());
        let h = make(config(), backend.clone(), net.clone());
        let mut r = Recorder::default();
        let mut q = Request {
            file: Some(file("My Hub.lua")),
            ..Default::default()
        };
        match mode {
            "url" => {
                q.file = None;
                q.url = Some("https://github.com/u/r/blob/main/Cool%20Hub.lua".into());
            }
            "names" => {
                q.annotate = true;
                q.names_file = Some(InputFile {
                    filename: "names.json".into(),
                    size: 30,
                    url: "https://cdn.discordapp.com/names.json".into(),
                });
            }
            "trace" => q.trace = true,
            "sandbox" => q.sandbox = true,
            "moonveil" => q.moonveil = true,
            _ => {}
        }
        h.run(&mut r, q).await.unwrap();
        assert!(r.accepted);
        {
            let seen = backend.seen.lock().unwrap();
            assert_eq!(seen.len(), 1);
            assert_eq!(seen[0].0, b"-- protected");
            assert_eq!(seen[0].1.annotate, mode == "names");
            assert_eq!(seen[0].1.trace, mode == "trace");
            assert_eq!(seen[0].1.sandbox_only, mode == "sandbox");
            assert_eq!(seen[0].1.moonveil, mode == "moonveil");
        }
        let (d, c) = r.final_.unwrap();
        assert!(d.ok, "{mode}: {}", d.text);
        let c = c.unwrap();
        assert!(c.title.contains("Result ready"));
        assert!(d.text.contains("allowed to analyze"));
        let names = d.files.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>();
        match mode {
            "file" => {
                assert_eq!(names, vec!["My_Hub.deobf.lua", "My_Hub_verification.txt"]);
                assert_eq!(d.files[0].1, b"print(\"hi\")\n");
                assert!(d.files[1].1.starts_with(b"Status: BYTECODE CHECK PASSED\n"));
                assert!(
                    d.text.contains("Bacon Guard v0.42.0")
                        && d.text.contains("3/3 functions identical")
                );
            }
            "url" => {
                assert_eq!(names[0], "Cool_20Hub.deobf.lua");
                assert_eq!(
                    net.calls.lock().unwrap()[0].0,
                    "https://raw.githubusercontent.com/u/r/main/Cool%20Hub.lua"
                );
                let initial = &r.cards[0];
                assert!(initial.title.ends_with("Fetching script"));
                assert!(initial.description.contains("✅ Validating URL\n**⠋ Downloading content**\n▫️ Detecting obfuscator\n▫️ Preparing download"));
                assert!(c.description.ends_with("✅ Validating URL\n✅ Downloading content\n✅ Detecting obfuscator\n✅ Preparing download"));
            }
            "names" => {
                assert_eq!(
                    backend.seen.lock().unwrap()[0].1.names.as_ref().unwrap()["12:34"],
                    "playerList"
                );
                assert_eq!(names[0], "My_Hub_annotated.lua");
                assert!(d.text.contains("PARTIAL"));
            }
            "trace" => {
                assert_eq!(
                    names,
                    vec![
                        "My_Hub.deobf.lua",
                        "My_Hub_verification.txt",
                        "My_Hub_trace.txt",
                        "My_Hub_payloads.zip"
                    ]
                );
                assert!(d
                    .text
                    .contains("2 request(s) to `api.example.com`, `cdn.example.org`"));
                assert!(!String::from_utf8_lossy(&d.files[2].1).contains("@@"));
            }
            "sandbox" => {
                assert_eq!(names, vec!["My_Hub_trace.txt", "My_Hub_payloads.zip"]);
                assert!(d.text.contains("nothing was deobfuscated"));
                assert!(!d.text.contains("Bytecode"));
                assert!(!d.text.contains("Detected"));
            }
            "moonveil" => {
                assert_eq!(names, vec!["My_Hub_reconstructed.lua"]);
                assert!(d.text.contains("NOT the original source"));
                assert!(!d.text.contains("Bytecode"));
            }
            _ => {}
        }
    }
}

#[tokio::test]
async fn handler_blocks_unlisted_hosts_and_forwards_url_fetch_configuration() {
    let mut cfg = bot::Config::from_env(&BTreeMap::from([
        ("COOLDOWN".into(), "0".into()),
        ("MAX_MB".into(), "2".into()),
        (
            "URL_HOSTS".into(),
            "example.com raw.githubusercontent.com".into(),
        ),
    ]))
    .unwrap();
    cfg.url_timeout = Duration::from_secs(7);
    let backend = Arc::new(FakeBackend::default());
    let net = Arc::new(FakeNetwork::default());
    let h = make(cfg, backend.clone(), net.clone());

    let mut blocked = Recorder::default();
    h.run(
        &mut blocked,
        Request {
            url: Some("https://denied.example/x.lua".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(!blocked.accepted && !blocked.final_.unwrap().0.ok);
    assert!(net.calls.lock().unwrap().is_empty());

    let mut allowed = Recorder::default();
    h.run(
        &mut allowed,
        Request {
            url: Some("https://github.com/u/r/blob/main/Cool.lua".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(allowed.final_.unwrap().0.ok);
    assert_eq!(
        *net.calls.lock().unwrap(),
        vec![(
            "https://raw.githubusercontent.com/u/r/main/Cool.lua".into(),
            2 * 1024 * 1024,
            vec!["example.com".into(), "raw.githubusercontent.com".into()],
            Duration::from_secs(7),
        )]
    );
    assert_eq!(backend.seen.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn input_conflicts_and_access_restrictions_never_download() {
    let backend = Arc::new(FakeBackend::default());
    let net = Arc::new(FakeNetwork::default());
    let h = make(config(), backend.clone(), net.clone());
    let base = Request {
        file: Some(file("x.lua")),
        ..Default::default()
    };
    let mut cases = vec![
        Request::default(),
        Request {
            url: Some("https://example.com/x".into()),
            ..base.clone()
        },
        Request {
            trace: true,
            sandbox: true,
            ..base.clone()
        },
        Request {
            sandbox: true,
            annotate: true,
            ..base.clone()
        },
        Request {
            sandbox: true,
            names_file: Some(InputFile {
                filename: "names.json".into(),
                size: 30,
                url: "https://cdn.discordapp.com/names.json".into(),
            }),
            ..base.clone()
        },
        Request {
            moonveil: true,
            sandbox: true,
            ..base.clone()
        },
        Request {
            moonveil: true,
            annotate: true,
            ..base.clone()
        },
        Request {
            moonveil: true,
            names_file: Some(InputFile {
                filename: "names.json".into(),
                size: 30,
                url: "https://cdn.discordapp.com/names.json".into(),
            }),
            ..base.clone()
        },
        Request {
            file: Some(file("evil.exe")),
            ..Default::default()
        },
    ];
    for url in [
        "http://127.0.0.1/",
        "http://169.254.169.254/",
        "https://x.io:8443/",
        "https://u:p@x.io/",
    ] {
        cases.push(Request {
            url: Some(url.into()),
            ..Default::default()
        });
    }
    for q in cases {
        let diagnostic = if q.trace && q.sandbox {
            Some("Pick one: `trace`")
        } else if q.sandbox && (q.annotate || q.names_file.is_some()) {
            Some("`sandbox` does not deobfuscate")
        } else if q.moonveil && q.sandbox {
            Some("`!uwu sandbox`")
        } else if q.moonveil && (q.annotate || q.names_file.is_some()) {
            Some("`annotate` and `names` do not apply")
        } else {
            None
        };
        let mut r = Recorder::default();
        h.run(&mut r, q).await.unwrap();
        assert!(!r.accepted);
        let delivery = r.final_.unwrap().0;
        assert!(!delivery.ok);
        assert!(delivery.files.is_empty());
        if let Some(needle) = diagnostic {
            assert!(delivery.text.contains(needle), "{}", delivery.text);
        }
    }
    assert!(net.calls.lock().unwrap().is_empty());
    assert!(backend.seen.lock().unwrap().is_empty());
    let mut cfg = config();
    cfg.allowed_users.insert(5);
    cfg.allowed_guilds.insert(900);
    let h = make(cfg, backend, net);
    for (user, guild, allowed) in [
        (1, None, false),
        (5, None, true),
        (77, Some(900), true),
        (78, Some(901), false),
    ] {
        assert_eq!(
            h.allowed(Identity {
                user,
                guild,
                upload_limit: 1000
            }),
            allowed
        );
    }
}

#[tokio::test]
async fn errors_stop_card_and_hide_host_paths_and_internal_details() {
    for code in [
        "unsupported",
        "internal",
        "queue_full",
        "user_busy",
        "cancelled",
    ] {
        let b = Arc::new(FakeBackend {
            error: Some(code),
            ..Default::default()
        });
        let h = make(config(), b, Arc::new(FakeNetwork::default()));
        let mut r = Recorder::default();
        h.run(
            &mut r,
            Request {
                file: Some(file("x.lua")),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let (d, c) = r.final_.unwrap();
        assert!(!d.ok);
        assert!(d.files.is_empty());
        assert!(c.unwrap().title.contains("Failed"));
        assert!(!d.text.contains("/srv/secret"));
        if code == "internal" {
            assert!(!d.text.contains("operation 77"));
        }
        if code == "unsupported" {
            assert!(d.text.contains("won't guess"));
            assert!(d.text.contains("operation 77"));
        }
    }
    let b = Arc::new(FakeBackend {
        error: Some("unsupported"),
        ..Default::default()
    });
    let h = make(config(), b, Arc::new(FakeNetwork::default()));
    let mut r = Recorder::default();
    h.run(
        &mut r,
        Request {
            url: Some("https://x.io/x.lua".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(r.final_.unwrap().0.text.contains("Downloaded the file"));
}

#[tokio::test]
async fn timeout_releases_admission_and_cooldown_is_after_completion() {
    let backend = Arc::new(FakeBackend {
        delay: Duration::from_millis(10),
        ..Default::default()
    });
    let mut cfg = config();
    cfg.max_wait = Duration::from_millis(2);
    cfg.cooldown = Duration::from_secs(60);
    let h = make(cfg, backend, Arc::new(FakeNetwork::default()));
    let mut r = Recorder::default();
    let q = Request {
        file: Some(file("x.lua")),
        ..Default::default()
    };
    h.run(&mut r, q.clone()).await.unwrap();
    assert!(r.final_.unwrap().0.text.contains("took too long"));
    let mut r = Recorder::default();
    h.run(&mut r, q).await.unwrap();
    assert!(r.final_.unwrap().0.text.contains("Please wait"));
}

#[tokio::test]
async fn plain_status_includes_queue_and_engine_stage() {
    let backend = Arc::new(FakeBackend {
        delay: Duration::from_millis(1),
        ..Default::default()
    });
    let mut cfg = config();
    cfg.progress_card = false;
    let h = make(cfg, backend, Arc::new(FakeNetwork::default()));
    let mut r = Recorder::default();
    h.run(
        &mut r,
        Request {
            file: Some(file("x.lua")),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(r.cards.is_empty());
    assert!(r.texts.iter().any(|s| s.contains("Queued (position 1)")));
    assert!(r.final_.unwrap().1.is_none());
}

#[test]
fn command_definitions_and_chat_source_precedence() {
    let cmds = bot::commands()
        .into_iter()
        .map(|c| serde_json::to_value(c).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(cmds.len(), 3);
    assert_eq!(cmds[0]["name"], "deobf");
    assert_eq!(cmds[1]["name"], "deobf-info");
    assert_eq!(cmds[2]["name"], "Deobfuscate file");
    assert_eq!(cmds[2]["type"], 3);
    for command in &cmds {
        let name = command["name"].as_str().unwrap();
        assert!((1..=32).contains(&name.chars().count()));
        assert!(
            command["description"]
                .as_str()
                .unwrap_or("")
                .chars()
                .count()
                <= 100
        );
    }
    for (name, kind) in [
        ("file", 11),
        ("url", 3),
        ("names", 11),
        ("private", 5),
        ("annotate", 5),
        ("trace", 5),
        ("sandbox", 5),
    ] {
        let option = cmds[0]["options"]
            .as_array()
            .unwrap()
            .iter()
            .find(|o| o["name"] == name)
            .unwrap();
        assert_eq!(option["type"], kind);
        assert_eq!(option["required"], false);
    }
    for o in cmds[0]["options"].as_array().unwrap() {
        assert_eq!(o["required"], false);
        assert!(o["description"].as_str().unwrap().len() <= 100);
    }
    let q = bot::chat_request("TRACE https://x.io/trace.lua", &[], None, false);
    assert!(q.trace);
    assert!(!q.sandbox);
    assert_eq!(q.url.as_deref(), Some("https://x.io/trace.lua"));
    let q = bot::chat_request("https://x.io/trace.lua", &[], None, false);
    assert!(!q.trace);
    let atts = [file("pic.png"), file("hub.lua")];
    let q = bot::chat_request("https://x.io/ignored.lua", &atts, None, true);
    assert_eq!(q.file.unwrap().filename, "hub.lua");
    assert!(q.url.is_none());
    assert!(q.moonveil);
    let q = bot::chat_request(
        "sandbox",
        &[],
        Some((&atts, "https://x.io/ignored.lua")),
        false,
    );
    assert!(q.sandbox);
    assert_eq!(q.file.unwrap().filename, "hub.lua");
    let (file, url) = bot::select_message_source(&[file("pic.png")], "https://x.io/hub.lua", false);
    assert!(file.is_none());
    assert_eq!(url.as_deref(), Some("https://x.io/hub.lua"));
}

#[test]
fn upload_budget_keeps_lua_and_verification_distinctions() {
    let mut result = output();
    result.info["banner"]["protection"] = json!("x".repeat(1900));
    let d = bot::package("My_Script", &result, 1.0, 64 * 1024 + 32).unwrap();
    assert_eq!(d.files.len(), 1);
    assert_eq!(d.files[0].1, b"print(\"hi\")\n");
    assert!(d.text.contains("Verification: BYTECODE CHECK PASSED"));
    assert!(d.text.contains("verification report did not fit"));
    assert!(d.text.contains("allowed to analyze"));
    assert!(d.text.chars().count() <= 1900);
    result.info["verification"]["delivered_syntax"] = json!("failed");
    assert!(bot::package("x", &result, 1.0, bot::DM_UPLOAD_LIMIT)
        .unwrap()
        .text
        .contains("Verification: FAILED"));
    result.info["verification"]["delivered_syntax"] = json!("passed");
    result.info["verification"]["raw_applies_to_delivery"] = json!(false);
    assert!(bot::package("x", &result, 1.0, bot::DM_UPLOAD_LIMIT)
        .unwrap()
        .text
        .contains("Verification: PARTIAL"));
}

#[test]
fn failed_verification_delivery_keeps_lua_and_failed_report_together() {
    let mut result = output();
    result.text = Some("print(\"failed fixture\")\n".into());
    result.info["verification"]["delivered_syntax"] = json!("failed");

    let delivery = bot::package("failed", &result, 1.0, bot::DM_UPLOAD_LIMIT).unwrap();
    assert!(delivery.ok);
    assert_eq!(
        delivery
            .files
            .iter()
            .map(|f| f.0.as_str())
            .collect::<Vec<_>>(),
        ["failed.deobf.lua", "failed_verification.txt"]
    );
    assert_eq!(delivery.files[0].1, b"print(\"failed fixture\")\n");
    assert!(delivery.files[1].1.starts_with(b"Status: FAILED\n"));
    assert!(delivery.text.contains("Verification: FAILED"));
}

#[test]
fn zipped_results_roundtrip_and_unuploadable_result_is_refused() {
    let mut result = output();
    let body = "print(1)\n".repeat(10000);
    result.text = Some(body.clone());
    let d = bot::package("x", &result, 1.0, 64 * 1024 + 1000).unwrap();
    assert_eq!(d.files[0].0, "x.deobf.lua.zip");
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&d.files[0].1)).unwrap();
    let mut decoded = String::new();
    zip.by_name("x.deobf.lua")
        .unwrap()
        .read_to_string(&mut decoded)
        .unwrap();
    assert_eq!(decoded, body);
    let d = bot::package("x", &result, 1.0, 64 * 1024 + 1).unwrap();
    assert!(!d.ok);
    assert!(d.files.is_empty());
    assert!(d.text.contains("too large"));
}

#[test]
fn hostile_hostnames_and_report_wording() {
    let text = messages::host_list(
        json!([
            "evil.example",
            "**bold**",
            "<@123456789>",
            "@everyone",
            "[click](x",
            "a`b",
            "xn--80ak6aa92e.com"
        ])
        .as_array()
        .unwrap(),
        true,
    );
    assert!(text.contains("`evil.example`"));
    assert!(text.contains("5 host name(s) with unusual characters"));
    assert!(!text.contains("@everyone"));
    let text = messages::explain_fetch("loader", "x.", 3.0);
    assert!(text.contains("but x. That"));
    assert!(!text.contains(".."));
    let text = messages::explain_fetch("loader", "", 3.0);
    assert!(text.contains("but it did not give the script. That link is signed"));
    let text = messages::explain_fetch("loader", "HTTP 403...", 3.0);
    assert!(text.contains("but HTTP 403. That link is signed"));
    assert!(!text.contains(".."));
}

fn wire_message() -> serenity::all::Message {
    let mut m = serenity::all::Message::default();
    m.id = serenity::all::MessageId::new(5000);
    m.channel_id = serenity::all::ChannelId::new(777);
    m.author.id = serenity::all::UserId::new(123);
    m.content = "!uwu".into();
    m
}
fn interaction() -> serenity::all::CommandInteraction {
    serde_json::from_value(json!({"id":"100","application_id":"200","type":2,"data":{"id":"300","name":"deobf","type":1,"options":[]},"channel_id":"777","user":{"id":"123","username":"tester","discriminator":"0","avatar":null},"token":"unit-test-interaction","version":1,"locale":"en-US","entitlements":[],"attachment_size_limit":10485760,"authorizing_integration_owners":{}})).unwrap()
}

#[test]
fn serenity_slash_options_and_context_menu_resolved_sources() {
    let mut command = interaction();
    let att:serenity::all::Attachment=serde_json::from_value(json!({"id":"31337","filename":"My Hub.lua","size":100,"url":"https://cdn.discordapp.com/x.lua","proxy_url":"https://media.discordapp.net/x.lua"})).unwrap();
    let mut names = att.clone();
    names.id = serenity::all::AttachmentId::new(7);
    names.filename = "names.json".into();
    command
        .data
        .resolved
        .attachments
        .insert(att.id, att.clone());
    command.data.resolved.attachments.insert(names.id, names);
    command.data.options=serde_json::from_value(json!([{"type":11,"name":"file","value":"31337"},{"type":11,"name":"names","value":"7"},{"type":5,"name":"annotate","value":true},{"type":5,"name":"trace","value":true},{"type":5,"name":"private","value":false}])).unwrap();
    let request = bot::interaction_request(&command).unwrap();
    assert_eq!(request.file.unwrap().filename, "My Hub.lua");
    assert_eq!(request.names_file.unwrap().filename, "names.json");
    assert!(request.annotate && request.trace && !request.sandbox);
    command.data.options=serde_json::from_value(json!([{"type":3,"name":"url","value":"https://x.io/x.lua"},{"type":5,"name":"sandbox","value":true}])).unwrap();
    let request = bot::interaction_request(&command).unwrap();
    assert_eq!(request.url.as_deref(), Some("https://x.io/x.lua"));
    assert!(request.sandbox);
    command.data.name = "Deobfuscate file".into();
    command.data.target_id = Some(serde_json::from_value(json!("5000")).unwrap());
    let mut message = wire_message();
    message.attachments = vec![att];
    command
        .data
        .resolved
        .messages
        .insert(message.id, message.clone());
    assert_eq!(
        bot::interaction_request(&command)
            .unwrap()
            .file
            .unwrap()
            .filename,
        "My Hub.lua"
    );
    message.attachments.clear();
    message.content = "script https://x.io/hub.lua".into();
    command
        .data
        .resolved
        .messages
        .insert(message.id, message.clone());
    assert_eq!(
        bot::interaction_request(&command).unwrap().url.as_deref(),
        Some("https://x.io/hub.lua")
    );
    message.content.clear();
    command.data.resolved.messages.insert(message.id, message);
    assert!(bot::interaction_request(&command)
        .unwrap_err()
        .message
        .contains("has no .lua"));
}
async fn discord_server(
    replies: Vec<(u16, String)>,
) -> (
    Arc<serenity::all::Http>,
    Arc<Mutex<Vec<String>>>,
    tokio::task::JoinHandle<()>,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let seen = calls.clone();
    let task = tokio::spawn(async move {
        for (status, body) in replies {
            let (mut stream, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept())
                .await
                .unwrap()
                .unwrap();
            let mut bytes = Vec::new();
            let mut chunk = [0; 8192];
            loop {
                let n = stream.read(&mut chunk).await.unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&chunk[..n]);
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]);
                    let length = headers
                        .lines()
                        .find_map(|l| {
                            l.split_once(':')
                                .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                                .and_then(|(_, v)| v.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if bytes.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            seen.lock()
                .unwrap()
                .push(String::from_utf8_lossy(&bytes).into_owned());
            let response=format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
            stream.write_all(response.as_bytes()).await.unwrap();
        }
    });
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    let http = serenity::http::HttpBuilder::new("unit-test")
        .application_id(serenity::all::ApplicationId::new(200))
        .client(client)
        .proxy(format!("http://{address}"))
        .ratelimiter_disabled(true)
        .build();
    (Arc::new(http), calls, task)
}
fn json_body(request: &str) -> serde_json::Value {
    let body = request.split_once("\r\n\r\n").unwrap().1;
    let body = if body.starts_with('{') {
        body
    } else {
        body.split_once("name=\"payload_json\"")
            .unwrap()
            .1
            .split_once("\r\n\r\n")
            .unwrap()
            .1
            .split_once("\r\n--")
            .unwrap()
            .0
    };
    serde_json::from_str(body).unwrap()
}
fn wire_reply() -> String {
    serde_json::to_string(&wire_message()).unwrap()
}

#[tokio::test]
async fn actual_serenity_interaction_private_and_public_defer_and_upload() {
    for private in [true, false] {
        let (http, calls, task) =
            discord_server(vec![(204, String::new()), (200, wire_reply())]).await;
        let mut r = bot::InteractionResponder::new(http, interaction(), private);
        r.accept().await.unwrap();
        r.finish(
            Delivery {
                text: "result @everyone".into(),
                files: vec![("script.lua".into(), b"print(1)".to_vec())],
                ok: true,
            },
            None,
        )
        .await
        .unwrap();
        task.await.unwrap();
        let calls = calls.lock().unwrap();
        let callback = json_body(&calls[0]);
        assert_eq!(callback["type"], 5);
        assert_eq!(
            callback["data"]["flags"].as_u64().unwrap_or(0) & 64,
            if private { 64 } else { 0 }
        );
        assert_eq!(callback["data"]["allowed_mentions"]["parse"], json!([]));
        assert!(calls[1].starts_with("PATCH "));
        assert!(calls[1].contains("script.lua"));
        assert!(calls[1].contains("print(1)"));
        assert!(calls[1].contains("allowed_mentions"));
    }
}

#[tokio::test]
async fn actual_serenity_refusal_is_private_before_defer() {
    let (http, calls, task) = discord_server(vec![(204, String::new())]).await;
    let mut r = bot::InteractionResponder::new(http, interaction(), false);
    r.finish(
        Delivery {
            text: "Pick one".into(),
            ..Default::default()
        },
        None,
    )
    .await
    .unwrap();
    task.await.unwrap();
    let calls = calls.lock().unwrap();
    let body = json_body(&calls[0]);
    assert_eq!(body["type"], 4);
    assert_eq!(body["data"]["flags"], 64);
    assert_eq!(body["data"]["allowed_mentions"]["parse"], json!([]));
}

#[tokio::test]
async fn actual_serenity_chat_edits_same_reply_and_suppresses_mentions() {
    let (http, calls, task) = discord_server(vec![(200, wire_reply()), (200, wire_reply())]).await;
    let mut r = bot::MessageResponder::new(http, wire_message(), bot::DM_UPLOAD_LIMIT);
    r.status("Processing").await.unwrap();
    r.finish(
        Delivery {
            text: "Done".into(),
            files: vec![("x.lua".into(), b"print(1)".to_vec())],
            ok: true,
        },
        None,
    )
    .await
    .unwrap();
    task.await.unwrap();
    let calls = calls.lock().unwrap();
    let sent = json_body(&calls[0]);
    assert_eq!(sent["content"], "Processing");
    assert!(sent["message_reference"].is_object());
    assert_eq!(sent["allowed_mentions"]["parse"], json!([]));
    assert_eq!(sent["allowed_mentions"]["replied_user"], false);
    assert!(calls[1].starts_with("PATCH "));
    assert!(calls[1].contains("x.lua"));
}

#[tokio::test]
async fn actual_serenity_deleted_reference_retries_without_reference() {
    let (http, calls, task) = discord_server(vec![
        (
            404,
            "{\"code\":10008,\"message\":\"Unknown message\"}".into(),
        ),
        (200, wire_reply()),
        (200, wire_reply()),
    ])
    .await;
    let mut r = bot::MessageResponder::new(http, wire_message(), bot::DM_UPLOAD_LIMIT);
    r.status("Processing").await.unwrap();
    r.finish(
        Delivery {
            text: "Done".into(),
            files: vec![("finished.lua".into(), b"print(42)".to_vec())],
            ok: true,
        },
        None,
    )
    .await
    .unwrap();
    task.await.unwrap();
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert!(json_body(&calls[0]).get("message_reference").is_some());
    assert!(json_body(&calls[1]).get("message_reference").is_none());
    assert_eq!(json_body(&calls[1])["content"], "Processing");
    assert!(calls[2].starts_with("PATCH "));
    assert_eq!(
        json_body(&calls[2])["attachments"][0]["filename"],
        "finished.lua"
    );
    assert!(calls[2].contains("print(42)"));
}

#[tokio::test]
async fn actual_serenity_missing_attachment_permission_is_explained() {
    let (http, calls, task) = discord_server(vec![
        (
            403,
            "{\"code\":50013,\"message\":\"Missing Permissions\"}".into(),
        ),
        (200, wire_reply()),
    ])
    .await;
    let mut r = bot::MessageResponder::new(http, wire_message(), bot::DM_UPLOAD_LIMIT);
    r.finish(
        Delivery {
            text: "Done".into(),
            files: vec![("x.lua".into(), b"x".to_vec())],
            ok: true,
        },
        None,
    )
    .await
    .unwrap();
    task.await.unwrap();
    let calls = calls.lock().unwrap();
    assert!(json_body(&calls[1])["content"]
        .as_str()
        .unwrap()
        .contains("Attach Files"));
}

#[tokio::test]
async fn actual_serenity_forbidden_reply_is_a_permission_result() {
    let (http, _, task) = discord_server(vec![(
        403,
        "{\"code\":50013,\"message\":\"Missing Permissions\"}".into(),
    )])
    .await;
    let mut r = bot::MessageResponder::new(http, wire_message(), bot::DM_UPLOAD_LIMIT);
    assert_eq!(
        r.status("Processing").await.unwrap_err().code,
        "discord_forbidden"
    );
    task.await.unwrap();
}

#[test]
fn names_utf8_bom_key_name_and_item_limits() {
    assert_eq!(
        namesfile::parse_names(b"\xef\xbb\xbf{\"12:p1\":\"target\"}").unwrap()["12:p1"],
        "target"
    );
    for data in [
        b"{".as_slice(),
        b"nope",
        b"null",
        b"{\"1234567:1\":\"x\"}",
        b"{\"1:p1000\":\"x\"}",
        b"{\"1:1\":4}",
        b"{\"1:1\":\"bad-name\"}",
        b"{\"1:1\":\"\"}",
    ] {
        assert!(namesfile::parse_names(data).is_err());
    }
    let many = (0..20001)
        .map(|n| format!("\"1:{n}\":\"v\""))
        .collect::<Vec<_>>()
        .join(",");
    assert!(namesfile::parse_names(format!("{{{many}}}").as_bytes())
        .unwrap_err()
        .message
        .contains("too many"));
}

#[tokio::test]
async fn advertised_and_actual_sizes_and_names_are_bounded() {
    let b = Arc::new(FakeBackend::default());
    let net = Arc::new(FakeNetwork::default());
    let h = make(config(), b.clone(), net.clone());
    for (name, size, names) in [
        ("x.lua", 0, false),
        ("x.lua", 9 * 1024 * 1024, false),
        ("x.json", NAMES_TOO_BIG, true),
    ] {
        let mut a = file(name);
        a.size = size;
        assert!(h.check_attachment(&a, names).is_err());
    }
    let net = Arc::new(FakeNetwork {
        data: vec![b'x'; 3 * 1024 * 1024 + 1],
        ..Default::default()
    });
    let h = make(config(), b.clone(), net);
    let mut r = Recorder::default();
    h.run(
        &mut r,
        Request {
            file: Some(file("x.lua")),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(r.final_.unwrap().0.text.contains("too large"));
    assert!(b.seen.lock().unwrap().is_empty());
    let net = Arc::new(FakeNetwork {
        names: b"{\"oops\":\"x\"}".to_vec(),
        ..Default::default()
    });
    let h = make(config(), b, net);
    let mut r = Recorder::default();
    h.run(
        &mut r,
        Request {
            file: Some(file("x.lua")),
            names_file: Some(InputFile {
                filename: "names.json".into(),
                size: 10,
                url: "https://cdn.discordapp.com/names.json".into(),
            }),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(r.final_.unwrap().0.text.contains("unknown key"));
}
const NAMES_TOO_BIG: usize = 256 * 1024 + 1;

#[tokio::test]
async fn failed_source_and_attachment_downloads_are_distinguished() {
    for source in ["file", "url"] {
        let h = make(
            config(),
            Arc::new(FakeBackend::default()),
            Arc::new(FakeNetwork {
                error: Some("http"),
                ..Default::default()
            }),
        );
        let mut r = Recorder::default();
        let q = if source == "file" {
            Request {
                file: Some(file("x.lua")),
                ..Default::default()
            }
        } else {
            Request {
                url: Some("https://x.io/x.lua".into()),
                ..Default::default()
            }
        };
        h.run(&mut r, q).await.unwrap();
        let (d, c) = r.final_.unwrap();
        assert!(d.text.contains(if source == "file" {
            "Failed to download"
        } else {
            "HTTP 403"
        }));
        if source == "url" {
            assert!(r.cards[0].description.contains("✅ Validating URL\n**⠋ Downloading content**\n▫️ Detecting obfuscator\n▫️ Preparing download"));
        }
        let c = c.unwrap();
        assert!(c.description.contains(if source == "url" {
            "✅ Validating URL\n❌ Downloading content\n▫️ Detecting obfuscator\n▫️ Preparing download"
        } else {
            "❌ Detecting obfuscator\n▫️ Preparing download"
        }));
    }
}

#[tokio::test]
async fn in_flight_rejection_and_dropped_task_release() {
    let backend = Arc::new(FakeBackend {
        delay: Duration::from_secs(10),
        ..Default::default()
    });
    let h = Arc::new(make(config(), backend, Arc::new(FakeNetwork::default())));
    let task_h = h.clone();
    let q = Request {
        file: Some(file("x.lua")),
        ..Default::default()
    };
    let task_q = q.clone();
    let task = tokio::spawn(async move { task_h.run(&mut Recorder::default(), task_q).await });
    tokio::time::sleep(Duration::from_millis(25)).await;
    let mut r = Recorder::default();
    h.run(&mut r, q.clone()).await.unwrap();
    assert!(r.final_.unwrap().0.text.contains("already have a file"));
    let mut invalid = Recorder::default();
    h.run(&mut invalid, Request::default()).await.unwrap();
    assert!(invalid
        .final_
        .unwrap()
        .0
        .text
        .contains("already have a file"));
    task.abort();
    let _ = task.await;
    let mut r = Recorder::default();
    let pending = h.run(&mut r, q);
    assert!(tokio::time::timeout(Duration::from_millis(5), pending)
        .await
        .is_err());
    assert!(r.accepted);
}

#[test]
fn dotenv_fixture_loading_does_not_overwrite_environment_values() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.fixture");
    std::fs::write(
        &path,
        "# comment\nexport MAX_MB=2\nWORKERS=3\nSAMPLE='a b'\nIGNORED\n",
    )
    .unwrap();
    let mut env = BTreeMap::from([("WORKERS".into(), "9".into())]);
    assert!(bot::load_dotenv(&path, &mut env));
    assert_eq!(env["WORKERS"], "9");
    assert_eq!(env["SAMPLE"], "a b");
    assert!(!bot::load_dotenv(
        &dir.path().join("missing.fixture"),
        &mut env
    ));
}

#[test]
fn card_spinner_unknown_stages_failure_and_timer_freezing() {
    let mut c = Card::new(false, None);
    let first = c.data();
    c.tick();
    assert_ne!(first.title, c.data().title);
    c.stage("classify");
    let before = c.data().description;
    c.stage("unknown");
    assert_eq!(before, c.data().description);
    std::thread::sleep(Duration::from_millis(120));
    c.fail();
    let failed = c.data();
    assert!(failed.description.contains("❌ Detecting obfuscator"));
    assert!(failed.description.contains("▫️ Preparing download"));
    std::thread::sleep(Duration::from_millis(120));
    c.begin("deliver");
    c.complete();
    assert_eq!(failed, c.data());
    let mut c = Card::new(false, Some("<a:spin:123456789012345678>".into()));
    assert!(c.data().title.starts_with("<a:spin:"));
    c.fail();
    assert!(c.data().description.contains("❌ Detecting obfuscator"));
}

#[test]
fn config_flags_hosts_prefix_spinner_and_info() {
    for word in ["0", "false", "no", "off", "tidak"] {
        let cfg = bot::Config::from_env(&BTreeMap::from([
            ("PREFIX_COMMANDS".into(), word.into()),
            ("PROGRESS_CARD".into(), word.into()),
        ]))
        .unwrap();
        assert!(!cfg.prefix_commands && !cfg.progress_card);
    }
    let cfg = bot::Config::from_env(&BTreeMap::from([
        ("PREFIX".into(), "?".into()),
        ("URL_HOSTS".into(), "A.com, b.org.".into()),
        ("SPINNER_EMOJI".into(), "<a:spin:123456789012345678>".into()),
        ("PREFIX_COMMANDS".into(), "off".into()),
    ]))
    .unwrap();
    assert_eq!(cfg.url_hosts, vec!["a.com", "b.org"]);
    let h = make(
        cfg,
        Arc::new(FakeBackend::default()),
        Arc::new(FakeNetwork::default()),
    );
    let text = h.info_text();
    assert!(text.contains("`?uwu`"));
    assert!(text.contains("disabled"));
    assert!(text.contains("allowed to analyze"));
    assert!(h.chat_help(true).contains("?moonveil"));
    assert!(bot::Config::from_env(&BTreeMap::from([(
        "SPINNER_EMOJI".into(),
        "@everyone".into()
    )]))
    .is_err());
}

#[test]
fn trace_payload_archives_and_upload_omission_notes() {
    let mut result = output();
    result.trace = Some(
        json!({"report":"@@NAME@@ @@STEM@@".repeat(10000),"payloads":[{"id":1,"ext":"lua","data":[112,114,105,110,116,40,49,41]}]}),
    );
    let d = bot::package("x", &result, 1.0, 64 * 1024 + 3000).unwrap();
    assert!(d.files.iter().any(|(n, _)| n == "x_trace.zip"));
    let trace = d.files.iter().find(|(n, _)| n == "x_trace.zip").unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&trace.1)).unwrap();
    let mut report = String::new();
    zip.by_name("x_trace.txt")
        .unwrap()
        .read_to_string(&mut report)
        .unwrap();
    assert_eq!(
        report,
        "@@NAME@@ @@STEM@@"
            .repeat(10000)
            .replace("@@NAME@@", "x")
            .replace("@@STEM@@", "x")
    );
    let payload = d.files.iter().find(|(n, _)| n == "x_payloads.zip").unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&payload.1)).unwrap();
    let mut data = Vec::new();
    zip.by_name("x_payload1.lua")
        .unwrap()
        .read_to_end(&mut data)
        .unwrap();
    assert_eq!(data, b"print(1)");
    let d = bot::package("x", &result, 1.0, 64 * 1024 + 32).unwrap();
    assert_eq!(d.files.len(), 1);
    assert!(d.text.contains("trace report is too large"));
    assert!(d.text.contains("payloads are too large"));
    result.trace.as_mut().unwrap()["payloads"] = json!([]);
    result.trace.as_mut().unwrap()["report"] = json!("@@NAME@@ @@STEM@@");
    let d = bot::package("x", &result, 1.0, bot::DM_UPLOAD_LIMIT).unwrap();
    assert_eq!(
        d.files.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
        ["x.deobf.lua", "x_verification.txt", "x_trace.txt"]
    );
    assert_eq!(d.files[2].1, b"x x");
}

#[test]
fn messages_scrub_paths_preserve_ratios_and_explain_only_public_details() {
    for (raw,want) in [
        ("/tmp/tmpab12.lua(8,14): SyntaxError: boom","line 8:14: SyntaxError: boom"),
        ("C:/Users/John Smith/AppData/Local/Temp/bgdeobf_job_x/tmpy3p62yo2.lua(8,14): SyntaxError: boom","line 8:14: SyntaxError: boom"),
        ("C:\\Users\\Someone~1\\AppData\\Local\\Temp\\x\\dump.lua:12: attempt to index nil","line 12: attempt to index nil"),
        ("[Errno 8] Exec format error: '/opt/app/luau_bin/luau'","[Errno 8] Exec format error: '<path>'"),
        ("plain message with no paths (and/or a ratio 3/4)","plain message with no paths (and/or a ratio 3/4)"),
    ] {assert_eq!(messages::scrub(raw),want);}
    assert!(messages::explain("unsupported", "").contains("won't guess"));
    assert!(messages::explain("sandbox", "x").contains("Technical detail: x"));
    assert!(!messages::explain("internal", "Traceback").contains("Traceback"));
    assert!(messages::explain("image", "").contains("image"));
    assert!(!messages::explain("image", "").contains("instructions"));
    let mut i = json!({"banner":{"version":"1","protection":"P","build":"b"},"protos":2,"records":3,"lines":4,"fidelity":{"functions":2,"identical":1,"reordered":0,"cosmetic":1,"differing":[]},"warnings":["w"]});
    let lines = messages::verdict(&i);
    assert!(lines
        .iter()
        .any(|s| s.contains("1 with a cosmetic difference")));
    assert_eq!(lines.last().unwrap(), "Note: w");
    i["fidelity"] =
        json!({"functions":70,"identical":69,"reordered":1,"cosmetic":0,"differing":[]});
    let lines = messages::verdict(&i);
    let line = lines
        .iter()
        .find(|s| s.starts_with("Bytecode check"))
        .unwrap();
    assert!(line.starts_with("Bytecode check: 69/70 functions identical, 1 with the same instructions in a different order"));
    assert!(!line.replace("different", "").contains("differ"));
    let mut i =
        json!({"banner":null,"protos":1,"records":1,"lines":1,"warnings":[],"syntax_ok":true});
    assert!(!messages::verdict(&i)
        .iter()
        .any(|s| s.contains("Annotated")));
    i["annotated"] = json!(true);
    assert!(messages::verdict(&i).iter().any(|s| [
        "Annotated copy",
        "names file",
        "not runnable",
        "clean copy"
    ]
    .iter()
    .all(|text| s.contains(text))));
    i["warnings"] =
        json!(["the result has a syntax error: /srv/bots/secret/tmp123.lua(8,14): SyntaxError: x"]);
    let lines = messages::verdict(&i);
    assert_eq!(
        lines.last().unwrap(),
        "Note: the result has a syntax error: line 8:14: SyntaxError: x"
    );
    assert!(!lines.join("\n").contains("secret"));
    assert!(!messages::explain(
        "sandbox",
        "unpacking failed: /srv/bots/secret/tmp123.lua:3: boom"
    )
    .contains("secret"));
}

#[test]
fn host_and_trace_messages_keep_original_counts_and_filter_untrusted_names() {
    for (name, ok) in [
        ("example.com", true),
        ("a-b.c0.example.org", true),
        ("xn--80ak6aa92e.com", true),
        ("**b**", false),
        ("<@123>", false),
        ("@everyone", false),
        ("a`b", false),
        ("a b", false),
        ("[::1]", false),
        ("", false),
    ] {
        let out = messages::host_list(&[json!(name)], true);
        assert_eq!(out.contains(&format!("`{name}`")), ok);
        if !ok && !name.is_empty() {
            assert!(!out.contains(name));
        }
    }
    for (names, md, want) in [
        (
            json!(["a.io", "@x", "<@1>"]),
            true,
            "`a.io` and 2 host name(s) with unusual characters",
        ),
        (
            json!(["@x"]),
            false,
            "1 host name(s) with unusual characters",
        ),
        (json!([]), false, "unknown hosts"),
        (
            json!(["a.io", "b.io", "c.io", "d.io", "e.io"]),
            false,
            "a.io, b.io, c.io, d.io ...",
        ),
    ] {
        assert_eq!(messages::host_list(names.as_array().unwrap(), md), want);
    }
    let trace = json!({"events":40,"requests":2,"domains":["api.example.com","*x*"],"remotes":1,"payloads":1,"instances":3,"errors":1,"findings":["Kicks the player","Kicks the player"],"finished":false,"reason":"the real-time limit was reached"});
    let md = messages::trace_lines(&trace, true);
    assert_eq!(md[0],"Fake Roblox run: 40 events, 2 request(s) to `api.example.com` and 1 host name(s) with unusual characters, 1 remote call(s), 1 text(s) captured (loadstring / files / requests), 3 instances created, 1 error(s)");
    assert_eq!(md[1], "Worth a look: Kicks the player");
    assert!(md[2].ends_with("(the log may be incomplete)"));
    assert!(!messages::trace_lines(&trace, false)[0].contains('`'));
}

#[tokio::test]
async fn unexpected_backend_panic_returns_red_card_and_releases_the_user() {
    let backend = Arc::new(FakeBackend {
        panic: true,
        ..Default::default()
    });
    let h = make(config(), backend, Arc::new(FakeNetwork::default()));
    for _ in 0..2 {
        let mut r = Recorder::default();
        h.run(
            &mut r,
            Request {
                file: Some(file("x.lua")),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let (d, c) = r.final_.unwrap();
        assert!(!d.ok && d.files.is_empty());
        assert!(d.text.contains("internal error"));
        assert!(!d.text.contains("secret"));
        assert!(c.unwrap().title.starts_with('❌'));
    }
}

#[test]
fn card_source_steps_stage_labels_embed_and_initial_failure_match_reference() {
    let mut file = Card::new(false, None);
    let mut url = Card::new(true, None);
    let data = file.data();
    assert_eq!(data.footer, "0.0s elapsed");
    assert!(data.title.ends_with("Processing script"));
    let lines = |d: bgdeobf::card::CardData| {
        d.description
            .split_once("\n\n")
            .unwrap()
            .1
            .lines()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    assert_eq!(lines(url.data()).len(), lines(file.data()).len() + 2);
    url.begin("download");
    let steps = lines(url.data());
    assert_eq!(steps[0], "✅ Validating URL");
    assert!(steps[1].starts_with("**") && steps[1].contains("Downloading content"));
    assert!(steps[2..].iter().all(|s| s.starts_with("▫️")));
    for (stage, detail) in [
        ("analyse", "reading the loader"),
        ("unpack", "opening the VM"),
        ("classify", "recognizing handlers"),
        ("decompile", "rebuilding the code"),
        ("verify", "verifying the result"),
        ("sandbox", "running it in the fake Roblox"),
        ("clean", "cleaning up the result"),
    ] {
        file.stage(stage);
        let active = lines(file.data())
            .into_iter()
            .filter(|s| s.starts_with("**"))
            .collect::<Vec<_>>();
        assert_eq!(active.len(), 1);
        assert!(
            active[0].contains("Detecting obfuscator")
                && active[0].ends_with(&format!(" · {detail}"))
        );
    }
    let mut c = Card::new(false, None);
    c.fail();
    assert!(lines(c.data())[0].starts_with('❌'));
    let mut c = Card::new(false, None);
    c.stage("analyse");
    let embed = serde_json::to_value(c.data().embed()).unwrap();
    assert!(embed["title"]
        .as_str()
        .unwrap()
        .ends_with("Processing script"));
    assert!(embed["description"]
        .as_str()
        .unwrap()
        .contains("Detecting obfuscator"));
    assert!(embed["footer"]["text"]
        .as_str()
        .unwrap()
        .ends_with("elapsed"));
}

#[tokio::test]
async fn real_chat_adapter_ignores_bots_opt_out_other_prefixes_and_muted_channels() {
    let backend = Arc::new(FakeBackend::default());
    let net = Arc::new(FakeNetwork::default());
    for case in [
        "off",
        "off_moonveil",
        "bot",
        "other",
        "muted",
        "suffix",
        "renamed",
    ] {
        let mut cfg = config();
        let mut m = wire_message();
        let mut speak = true;
        match case {
            "off" => cfg.prefix_commands = false,
            "off_moonveil" => {
                cfg.prefix_commands = false;
                m.content = "!moonveil".into();
            }
            "bot" => m.author.bot = true,
            "other" => m.content = "hello everyone".into(),
            "muted" => speak = false,
            "suffix" => m.content = "!uwuu".into(),
            "renamed" => cfg.prefix = "?".into(),
            _ => unreachable!(),
        }
        let h = make(cfg, backend.clone(), net.clone());
        let (http, calls, task) = discord_server(vec![]).await;
        h.from_chat(http, m, bot::DM_UPLOAD_LIMIT, speak)
            .await
            .unwrap();
        task.await.unwrap();
        assert!(calls.lock().unwrap().is_empty());
    }
    assert!(backend.seen.lock().unwrap().is_empty());
    assert!(net.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn real_chat_adapter_usage_reply_fetch_moonveil_and_source_precedence() {
    for (content, reference, want) in [
        ("!uwu", None, "Usage: `!uwu`"),
        ("!uwu help", None, "Usage: `!uwu`"),
        ("!uwu ?", None, "Usage: `!uwu`"),
        ("!uwu what is this", None, "Usage: `!uwu`"),
        ("!uwu trace", None, "Usage: `!uwu`"),
        ("!moonveil", None, "Usage: `!moonveil`"),
        ("!moonveil help", None, "Usage: `!moonveil`"),
        (
            "!uwu",
            Some("https://x.io/fetched.lua"),
            "fetched.deobf.lua",
        ),
        (
            "!moonveil TRACE",
            Some("https://x.io/x.lua"),
            "x_reconstructed.lua",
        ),
    ] {
        let mut cfg = config();
        cfg.progress_card = false;
        let backend = Arc::new(FakeBackend {
            silent: true,
            ..Default::default()
        });
        let net = Arc::new(FakeNetwork::default());
        let h = make(cfg, backend.clone(), net.clone());
        let mut m = wire_message();
        m.content = content.into();
        let mut responses = vec![];
        if let Some(source) = reference {
            let mut original = wire_message();
            original.id = serenity::all::MessageId::new(4998);
            original.content = source.into();
            m.message_reference = Some(serenity::all::MessageReference::from(&original));
            responses.push((200, serde_json::to_string(&original).unwrap()));
            responses.push((200, wire_reply()));
            responses.push((200, wire_reply()));
        } else {
            responses.push((200, wire_reply()));
        }
        let (http, calls, task) = discord_server(responses).await;
        h.from_chat(http, m, bot::DM_UPLOAD_LIMIT, true)
            .await
            .unwrap();
        task.await.unwrap();
        let calls = calls.lock().unwrap();
        assert!(
            calls.iter().any(|c| c.contains(want)),
            "{content}: {calls:?}"
        );
        if reference.is_some() {
            assert!(calls[0].starts_with("GET "));
            let seen = backend.seen.lock().unwrap();
            assert_eq!(seen.len(), 1);
            assert_eq!(seen[0].1.moonveil, content.contains("moonveil"));
            assert_eq!(seen[0].1.trace, content.contains("TRACE"));
        } else {
            assert_eq!(calls.len(), 1);
            assert!(net.calls.lock().unwrap().is_empty());
            if content.starts_with("!uwu") {
                let body = json_body(&calls[0]);
                let usage = body["content"].as_str().unwrap();
                for phrase in ["raw url", "trace", "sandbox"] {
                    assert!(usage.contains(phrase), "{content}: {usage}");
                }
            }
        }
    }
}

#[tokio::test]
async fn real_interaction_adapter_info_context_and_slash_use_the_same_domain_flow() {
    for mode in [
        "info",
        "slash",
        "slash_public",
        "menu_url",
        "menu_file",
        "menu_none",
        "both",
        "neither",
    ] {
        let mut cfg = config();
        cfg.progress_card = false;
        let backend = Arc::new(FakeBackend {
            silent: true,
            ..Default::default()
        });
        let net = Arc::new(FakeNetwork::default());
        let h = make(cfg, backend.clone(), net.clone());
        let mut c = interaction();
        match mode {
            "info" => c.data.name = "deobf-info".into(),
            "slash" | "slash_public" => {
                c.data.options=serde_json::from_value(json!([{"name":"url","type":3,"value":"loadstring(game:HttpGet(\"https://pastebin.com/raw/AbCd1234\"))()"}])).unwrap();
                if mode == "slash_public" {
                    c.data.options.push(
                        serde_json::from_value(json!({"name":"private","type":5,"value":false}))
                            .unwrap(),
                    );
                }
            }
            "both" => {
                let a:serenity::all::Attachment=serde_json::from_value(json!({"id":"7","filename":"hub.lua","size":10,"url":"https://cdn.discordapp.com/x","proxy_url":"https://media.discordapp.net/x"})).unwrap();
                c.data.resolved.attachments.insert(a.id, a);
                c.data.options=serde_json::from_value(json!([{"name":"file","type":11,"value":"7"},{"name":"url","type":3,"value":"https://x.io/x.lua"}])).unwrap();
            }
            "menu_url" | "menu_file" | "menu_none" => {
                c.data.name = "Deobfuscate file".into();
                c.data.target_id = Some(serde_json::from_value(json!("5000")).unwrap());
                let mut m = wire_message();
                m.content = if mode == "menu_url" {
                    "script https://x.io/hub.lua"
                } else {
                    "nothing here"
                }
                .into();
                if mode == "menu_file" {
                    m.attachments=serde_json::from_value(json!([{"id":"7","filename":"pic.png","size":10,"url":"https://cdn.discordapp.com/pic","proxy_url":"https://media.discordapp.net/pic"},{"id":"8","filename":"hub.lua","size":100,"url":"https://cdn.discordapp.com/x","proxy_url":"https://media.discordapp.net/x"}])).unwrap();
                }
                c.data.resolved.messages.insert(m.id, m);
            }
            _ => {}
        }
        let runs = matches!(mode, "slash" | "slash_public" | "menu_url" | "menu_file");
        let responses = if mode == "menu_file" {
            vec![(204, String::new()), (200, wire_reply())]
        } else if runs {
            vec![
                (204, String::new()),
                (200, wire_reply()),
                (200, wire_reply()),
            ]
        } else {
            vec![(204, String::new())]
        };
        let (http, calls, task) = discord_server(responses).await;
        h.from_interaction(http, c).await.unwrap();
        task.await.unwrap();
        let calls = calls.lock().unwrap();
        let first = json_body(&calls[0]);
        if runs {
            assert_eq!(first["type"], 5);
            assert_eq!(
                first["data"]["flags"].as_u64().unwrap_or(0),
                if mode == "slash_public" { 0 } else { 64 }
            );
            let last = calls.last().unwrap();
            assert!(last.contains(if matches!(mode, "slash" | "slash_public") {
                "AbCd1234.deobf.lua"
            } else {
                "hub.deobf.lua"
            }));
            assert_eq!(backend.seen.lock().unwrap().len(), 1);
        } else {
            assert_eq!(first["type"], 4);
            assert_eq!(first["data"]["flags"], 64);
            assert!(first["data"]["content"]
                .as_str()
                .unwrap()
                .contains(match mode {
                    "info" => "Bacon Guard",
                    "both" => "Pick one",
                    "neither" => "Include a",
                    _ => "has no .lua",
                }));
            assert!(net.calls.lock().unwrap().is_empty());
        }
    }
}

struct StageBackend {
    queued: bool,
    stage: &'static str,
}
#[async_trait]
impl Backend for StageBackend {
    async fn submit(
        &self,
        _: u64,
        _: Vec<u8>,
        _: Options,
        progress: Option<Progress>,
    ) -> Result<Output> {
        let progress = progress.unwrap();
        if self.queued {
            progress("queued:1");
            tokio::time::sleep(Duration::from_millis(1300)).await;
        }
        progress(self.stage);
        tokio::time::sleep(if self.queued {
            Duration::from_millis(1300)
        } else {
            Duration::from_millis(5)
        })
        .await;
        Ok(output())
    }
}

#[tokio::test]
async fn timed_card_redraws_queue_spinner_verification_and_finishes_without_plain_status() {
    let h = Handler::new(
        config(),
        Arc::new(StageBackend {
            queued: true,
            stage: "verify",
        }),
        Arc::new(FakeNetwork::default()),
    );
    let mut r = Recorder::default();
    h.run(
        &mut r,
        Request {
            file: Some(file("My Hub.lua")),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(r.cards[0].footer, "0.0s elapsed");
    assert!(r.cards[0].description.contains("Detecting obfuscator"));
    assert!(r
        .cards
        .iter()
        .any(|d| d.description.contains("Waiting in the queue (position 1).")));
    assert!(r
        .cards
        .iter()
        .any(|d| d.description.contains("verifying the result")));
    assert!(r.cards.iter().any(|d| d.title != r.cards[0].title));
    assert!(r.texts.is_empty());
    let (d, card) = r.final_.unwrap();
    assert_eq!(d.files[0].0, "My_Hub.deobf.lua");
    assert!(d.text.starts_with("✅ Done"));
    let c = card.unwrap();
    assert!(c.title.starts_with('✅'));
    assert!(!c.description.contains("queue"));
    assert!(c
        .description
        .split_once("\n\n")
        .unwrap()
        .1
        .lines()
        .all(|s| s.starts_with('✅')));
}

#[tokio::test]
async fn plain_sandbox_stage_is_named_and_card_off_returns_no_embed() {
    let mut cfg = config();
    cfg.progress_card = false;
    let h = Handler::new(
        cfg,
        Arc::new(StageBackend {
            queued: false,
            stage: "sandbox",
        }),
        Arc::new(FakeNetwork::default()),
    );
    let mut r = Recorder::default();
    h.run(
        &mut r,
        Request {
            file: Some(file("x.lua")),
            trace: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(r
        .texts
        .iter()
        .any(|s| s.contains("running it in the fake Roblox")));
    assert!(r.cards.is_empty());
    assert!(r.final_.unwrap().1.is_none());
}

#[test]
fn completed_card_ticks_all_steps_and_freezes_the_elapsed_time() {
    let mut c = Card::new(false, None);
    c.begin("detect");
    let first = c.data().footer;
    std::thread::sleep(Duration::from_millis(120));
    assert_ne!(c.data().footer, first);
    c.complete();
    let done = c.data();
    assert!(done.title.starts_with('✅') && done.footer.starts_with("Finished in "));
    assert!(done
        .description
        .split_once("\n\n")
        .unwrap()
        .1
        .lines()
        .all(|s| s.starts_with('✅')));
    std::thread::sleep(Duration::from_millis(120));
    c.tick();
    c.fail();
    assert_eq!(c.data(), done);
}

#[tokio::test]
async fn every_fetch_error_reaches_chat_with_its_original_explanation() {
    for (code, needle) in [
        ("html", "web page"),
        ("too_big", "too large"),
        ("http", "HTTP 403"),
        ("timeout", "too long"),
        ("blocked", "was rejected"),
        ("empty", "empty"),
        ("net", "Couldn't fetch"),
        ("loader", "only runs inside Roblox"),
    ] {
        let h = make(
            config(),
            Arc::new(FakeBackend::default()),
            Arc::new(FakeNetwork {
                error: Some(code),
                ..Default::default()
            }),
        );
        let mut r = Recorder::default();
        h.run(
            &mut r,
            Request {
                url: Some("https://example.com/x.lua".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let d = r.final_.unwrap().0;
        assert!(d.text.contains(needle), "{code}: {}", d.text);
        assert!(d.files.is_empty());
    }
}

#[test]
fn all_uwu_parser_cases_and_names_file_boundaries() {
    for (content, args) in [
        ("!uwu", Some("")),
        ("!uwu   ", Some("")),
        ("!uwu https://a.b/c.lua", Some("https://a.b/c.lua")),
        ("!UWU x", Some("x")),
        ("  !uwu x", Some("x")),
        (
            "!uwu\nloadstring(game:HttpGet(\"https://x.io/y.lua\"))()",
            Some("loadstring(game:HttpGet(\"https://x.io/y.lua\"))()"),
        ),
        ("!uwux", None),
        ("!uwuu", None),
        ("halo !uwu", None),
        ("!help", None),
        ("", None),
        ("uwu", None),
    ] {
        assert_eq!(bot::parse_command(content, "!", "uwu").as_deref(), args);
    }
    assert_eq!(
        bot::parse_command("?uwu a", "?", "uwu").as_deref(),
        Some("a")
    );
    assert_eq!(bot::parse_command("!uwu a", "?", "uwu"), None);
    assert_eq!(
        bot::parse_command("$$uwu", "$$", "uwu").as_deref(),
        Some("")
    );
    assert_eq!(
        bot::parse_command("ÉUWU x", "é", "uwu").as_deref(),
        Some("x")
    );
    let name = "a".repeat(40);
    let valid = format!("{{\"999999:999999\":\"{name}\",\"999999:p999\":\"_target1\"}}");
    let names = namesfile::parse_names(valid.as_bytes()).unwrap();
    assert_eq!(names["999999:999999"], name);
    assert_eq!(names["999999:p999"], "_target1");
    assert!(
        namesfile::parse_names(format!("{{\"1:1\":\"{}\"}}", "a".repeat(41)).as_bytes()).is_err()
    );
}

#[tokio::test]
async fn access_restrictions_reach_admission_before_download_and_cooldown() {
    let mut cfg = config();
    cfg.allowed_users.insert(5);
    cfg.allowed_guilds.insert(900);
    let net = Arc::new(FakeNetwork::default());
    let h = make(cfg, Arc::new(FakeBackend::default()), net.clone());
    for (user, guild, allowed) in [
        (123, None, false),
        (5, None, true),
        (77, Some(900), true),
        (78, Some(901), false),
        (79, None, false),
    ] {
        let count = net.calls.lock().unwrap().len();
        let mut r = Recorder {
            user,
            guild,
            ..Default::default()
        };
        h.run(
            &mut r,
            Request {
                file: Some(file("x.lua")),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(r.accepted, allowed);
        let (d, _) = r.final_.unwrap();
        assert_eq!(!d.files.is_empty(), allowed);
        if !allowed {
            assert!(d.text.contains("isn't open"));
            assert_eq!(net.calls.lock().unwrap().len(), count);
        }
    }
}

#[test]
fn partial_and_annotated_code_are_delivered_with_honest_reports() {
    let mut result = output();
    result.text = Some("print(\"recovered fixture\")\n".into());
    result.info["verification"]["raw_bytecode"] = json!("differences found");
    result.info["fidelity"]["identical"] = json!(2);
    result.info["fidelity"]["differing"] = json!([78]);
    let d = bot::package("ui", &result, 1.0, bot::DM_UPLOAD_LIMIT).unwrap();
    assert_eq!(
        d.files.iter().map(|f| f.0.as_str()).collect::<Vec<_>>(),
        ["ui.deobf.lua", "ui_verification.txt"]
    );
    assert_eq!(d.files[0].1, b"print(\"recovered fixture\")\n");
    let report = String::from_utf8_lossy(&d.files[1].1);
    assert!(report.starts_with("Status: PARTIAL\n") && report.contains("1 differ"));
    assert!(d.text.contains("Verification: PARTIAL") && !d.text.contains("DISPLAY TEXT ONLY"));
    result.text = Some("local x<@1:2> = 1\n".into());
    result.info["annotated"] = json!(true);
    result.info["verification"]["delivered_syntax"] = json!("not runnable");
    result.info["verification"]["raw_applies_to_delivery"] = json!(false);
    let d = bot::package("My_Script", &result, 1.0, bot::DM_UPLOAD_LIMIT).unwrap();
    assert_eq!(d.files[0].0, "My_Script_annotated.lua");
    assert_eq!(d.files[0].1, b"local x<@1:2> = 1\n");
    assert!(d.files[1].1.starts_with(b"Status: PARTIAL\n"));
    assert!(d.text.contains("Annotated copy") && !d.text.contains("syntax error"));
}

#[test]
fn semantic_proof_summary_covers_only_the_checked_delivery() {
    let mut r = output();
    r.info["verification"]["raw_bytecode"] = json!("differences found");
    r.info["verification"]["raw_semantic"] = json!("passed");
    r.info["fidelity"] = json!({"functions":2,"functions_recompiled":2,"identical":1,"reordered":0,"cosmetic":0,"semantic_equivalent":1,"differing":[]});
    let d = bot::package("fixture", &r, 1.0, bot::DM_UPLOAD_LIMIT).unwrap();
    let text = String::from_utf8_lossy(&d.files[1].1);
    assert!(
        text.starts_with("Status: RAW FIDELITY PASSED WITH PROOFS\n")
            && text.contains("2 accepted, 0 differ")
            && text.contains("1 proven equivalent"),
        "{text}"
    );
    assert!(d.text.contains("1/2 functions identical") && d.text.contains("1 proven equivalent"));
    for (key, value) in [
        ("raw_applies_to_delivery", json!(false)),
        ("raw_syntax", json!("unavailable")),
        ("raw_semantic", json!("incomplete")),
    ] {
        let mut modified = r.clone();
        modified.info["verification"][key] = value;
        assert!(
            bot::package("fixture", &modified, 1.0, bot::DM_UPLOAD_LIMIT)
                .unwrap()
                .files[1]
                .1
                .starts_with(b"Status: PARTIAL\n")
        );
    }
}

#[tokio::test]
async fn cached_reply_and_current_attachments_produce_one_reply_without_status_or_reference_fetch()
{
    for (cached, filename, want, valid) in [
        (false, "My Hub.lua", "My_Hub.deobf.lua", true),
        (true, "theirs.lua", "theirs.deobf.lua", true),
        (false, "virus.exe", "Send a", false),
    ] {
        let mut cfg = config();
        cfg.progress_card = false;
        let net = Arc::new(FakeNetwork::default());
        let h = make(
            cfg,
            Arc::new(FakeBackend {
                silent: true,
                ..Default::default()
            }),
            net.clone(),
        );
        let mut m = wire_message();
        let att:serenity::all::Attachment=serde_json::from_value(json!({"id":"7","filename":filename,"size":100,"url":"https://cdn.discordapp.com/script.lua","proxy_url":"https://media.discordapp.net/script.lua"})).unwrap();
        if cached {
            let mut original = wire_message();
            original.attachments = vec![att];
            m.referenced_message = Some(Box::new(original));
        } else {
            m.attachments = vec![att];
        }
        let (http, calls, task) = discord_server(vec![(200, wire_reply())]).await;
        h.from_chat(http, m, bot::DM_UPLOAD_LIMIT, true)
            .await
            .unwrap();
        task.await.unwrap();
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].starts_with("POST ") && calls[0].contains(want));
        if valid {
            assert!(calls[0].contains("print(\"hi\")"));
        }
        assert_eq!(!net.calls.lock().unwrap().is_empty(), valid);
    }
}

struct EnvelopeNetwork {
    calls: Mutex<Vec<String>>,
}
#[async_trait]
impl bgdeobf::fetch::Transport for EnvelopeNetwork {
    async fn resolve(&self, _: &str, port: u16) -> Result<Vec<std::net::SocketAddr>> {
        Ok(vec![std::net::SocketAddr::new(
            "8.8.8.8".parse().unwrap(),
            port,
        )])
    }
    async fn get(
        &self,
        url: &url::Url,
        _: &[std::net::SocketAddr],
        _: usize,
        _: Duration,
        _: &str,
    ) -> Result<bgdeobf::fetch::HttpReply> {
        self.calls.lock().unwrap().push(url.to_string());
        let link = "https://paste.example/api/stream/x";
        let body = if url.path() == "/raw/x" {
            let hex = link
                .bytes()
                .enumerate()
                .map(|(i, b)| format!("{:02x}", (usize::from(b) + 146 + i * 16) % 256))
                .collect::<String>();
            format!("local g,p,k,i,t,c,h=game,pcall,146,0,{{}},string.char,\"{hex}\";h:gsub(\"..\",function(x)t[i+1]=c((tonumber(x,16)-(k+i*16))%256)i=i+1 end);g:HttpGet(table.concat(t))").into_bytes()
        } else {
            b"-- real script\nprint(1)".to_vec()
        };
        Ok(bgdeobf::fetch::HttpReply {
            status: 200,
            body,
            ..Default::default()
        })
    }
}
#[async_trait]
impl Network for EnvelopeNetwork {
    async fn download(
        &self,
        url: &str,
        max: usize,
        hosts: &[String],
        timeout: Duration,
    ) -> Result<Vec<u8>> {
        bgdeobf::fetch::fetch_with(url, max, hosts, timeout, self).await
    }
}

#[tokio::test]
async fn chat_decodes_stage_one_and_submits_stream_bytes_not_loader_or_display_report() {
    let mut cfg = config();
    cfg.progress_card = false;
    let backend = Arc::new(FakeBackend {
        silent: true,
        ..Default::default()
    });
    let net = Arc::new(EnvelopeNetwork {
        calls: Mutex::new(Vec::new()),
    });
    let h = Handler::new(cfg, backend.clone(), net.clone());
    let mut message = wire_message();
    message.content = "!uwu https://paste.example/raw/x".into();
    let (http, calls, task) = discord_server(vec![(200, wire_reply()), (200, wire_reply())]).await;
    h.from_chat(http, message, bot::DM_UPLOAD_LIMIT, true)
        .await
        .unwrap();
    task.await.unwrap();
    assert_eq!(
        *net.calls.lock().unwrap(),
        [
            "https://paste.example/raw/x",
            "https://paste.example/api/stream/x"
        ]
    );
    assert_eq!(
        backend.seen.lock().unwrap()[0].0,
        b"-- real script\nprint(1)"
    );
    let calls = calls.lock().unwrap();
    assert!(
        calls.last().unwrap().contains("x.deobf.lua")
            && calls.last().unwrap().contains("x_verification.txt")
    );
}

#[test]
fn interaction_identity_uses_the_real_guild_and_falls_back_for_zero_upload_limit() {
    for (guild, advertised, expected) in [
        (None, 0, bot::DM_UPLOAD_LIMIT),
        (Some(900), 0, bot::DM_UPLOAD_LIMIT),
        (Some(901), 50 * 1024 * 1024, 50 * 1024 * 1024),
    ] {
        let mut command = interaction();
        command.guild_id = guild.map(serenity::all::GuildId::new);
        command.attachment_size_limit = advertised;
        let responder = bot::InteractionResponder::new(
            Arc::new(serenity::all::Http::new("unit-test")),
            command,
            true,
        );
        let id = responder.identity();
        assert_eq!(id.user, 123);
        assert_eq!(id.guild, guild);
        assert_eq!(id.upload_limit, expected);
    }
}

#[tokio::test]
async fn real_chat_adapter_custom_prefix_uses_message_guild_for_admission() {
    for (guild, allowed) in [(Some(900), true), (Some(901), false), (None, false)] {
        let mut cfg = config();
        cfg.prefix = "?".into();
        cfg.progress_card = false;
        cfg.guild_id = Some(999);
        cfg.allowed_guilds.insert(900);
        let backend = Arc::new(FakeBackend {
            silent: true,
            ..Default::default()
        });
        let net = Arc::new(FakeNetwork::default());
        let h = make(cfg, backend.clone(), net.clone());
        let mut message = wire_message();
        message.guild_id = guild.map(serenity::all::GuildId::new);
        message.content = "?uWu".into();
        message.attachments = serde_json::from_value(json!([{"id":"7","filename":"guild.lua","size":100,"url":"https://cdn.discordapp.com/script.lua","proxy_url":"https://media.discordapp.net/script.lua"}])).unwrap();
        let (http, calls, task) = discord_server(vec![(200, wire_reply())]).await;
        h.from_chat(http, message, bot::DM_UPLOAD_LIMIT, true)
            .await
            .unwrap();
        task.await.unwrap();
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        let payload = json_body(&calls[0]);
        if allowed {
            assert_eq!(payload["attachments"][0]["filename"], "guild.deobf.lua");
            assert!(calls[0].contains("print(\"hi\")"));
        } else {
            assert!(payload["content"].as_str().unwrap().contains("isn't open"));
            assert!(payload["attachments"].as_array().is_none_or(Vec::is_empty));
        }
        assert_eq!(backend.seen.lock().unwrap().len(), usize::from(allowed));
        assert_eq!(net.calls.lock().unwrap().len(), usize::from(allowed));
    }
}

#[tokio::test]
async fn real_chat_adapter_uses_cached_urls_and_handles_unusable_or_failed_references() {
    for case in [
        "cached_url",
        "cached_unusable",
        "fetched_unusable",
        "failed_get",
    ] {
        let mut cfg = config();
        cfg.progress_card = false;
        let backend = Arc::new(FakeBackend {
            silent: true,
            ..Default::default()
        });
        let net = Arc::new(FakeNetwork::default());
        let h = make(cfg, backend.clone(), net.clone());
        let mut message = wire_message();
        let mut original = wire_message();
        original.id = serenity::all::MessageId::new(4998);
        original.content = if case == "cached_url" {
            "script https://x.io/cached.lua"
        } else {
            "no script here"
        }
        .into();
        original.attachments = serde_json::from_value(json!([{"id":"7","filename":"picture.png","size":100,"url":"https://cdn.discordapp.com/picture.png","proxy_url":"https://media.discordapp.net/picture.png"}])).unwrap();
        message.message_reference = Some(serenity::all::MessageReference::from(&original));
        let mut responses = Vec::new();
        if case.starts_with("cached_") {
            message.referenced_message = Some(Box::new(original));
        } else if case == "failed_get" {
            responses.push((
                404,
                "{\"code\":10008,\"message\":\"Unknown message\"}".into(),
            ));
        } else {
            responses.push((200, serde_json::to_string(&original).unwrap()));
        }
        responses.push((200, wire_reply()));
        if case == "cached_url" {
            responses.push((200, wire_reply()));
        }
        let (http, calls, task) = discord_server(responses).await;
        h.from_chat(http, message, bot::DM_UPLOAD_LIMIT, true)
            .await
            .unwrap();
        task.await.unwrap();
        let calls = calls.lock().unwrap();
        assert_eq!(
            calls.iter().filter(|c| c.starts_with("GET ")).count(),
            usize::from(!case.starts_with("cached_"))
        );
        if !case.starts_with("cached_") {
            assert!(calls[0].starts_with("GET /api/v10/channels/777/messages/4998 "));
        }
        let last = json_body(calls.last().unwrap());
        if case == "cached_url" {
            assert_eq!(calls.len(), 2);
            assert!(calls[0].starts_with("POST ") && calls[1].starts_with("PATCH "));
            assert_eq!(last["attachments"][0]["filename"], "cached.deobf.lua");
            assert_eq!(net.calls.lock().unwrap()[0].0, "https://x.io/cached.lua");
            assert_eq!(backend.seen.lock().unwrap().len(), 1);
        } else {
            assert!(last["content"]
                .as_str()
                .unwrap()
                .starts_with("Usage: `!uwu`"));
            assert!(last["attachments"].as_array().is_none_or(Vec::is_empty));
            assert!(backend.seen.lock().unwrap().is_empty());
            assert!(net.calls.lock().unwrap().is_empty());
        }
    }
}

#[tokio::test]
async fn actual_serenity_final_edit_failure_sends_files_in_a_new_reply() {
    let (http, calls, task) = discord_server(vec![
        (200, wire_reply()),
        (
            404,
            "{\"code\":10008,\"message\":\"Unknown message\"}".into(),
        ),
        (200, wire_reply()),
    ])
    .await;
    let mut r = bot::MessageResponder::new(http, wire_message(), bot::DM_UPLOAD_LIMIT);
    r.status("Processing").await.unwrap();
    r.finish(
        Delivery {
            text: "Done".into(),
            files: vec![("recovered.lua".into(), b"print(43)".to_vec())],
            ok: true,
        },
        None,
    )
    .await
    .unwrap();
    task.await.unwrap();
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert!(calls[1].starts_with("PATCH ") && calls[2].starts_with("POST "));
    let payload = json_body(&calls[2]);
    assert_eq!(payload["content"], "Done");
    assert_eq!(payload["attachments"][0]["filename"], "recovered.lua");
    assert_eq!(payload["allowed_mentions"]["parse"], json!([]));
    assert!(calls[2].contains("print(43)"));
}

#[tokio::test]
async fn advertised_empty_and_large_attachments_are_private_refusals_without_defer_or_work() {
    for (size, needle) in [(0, "empty"), (config().max_bytes() + 1, "too large")] {
        let backend = Arc::new(FakeBackend::default());
        let net = Arc::new(FakeNetwork::default());
        let h = make(config(), backend.clone(), net.clone());
        let mut command = interaction();
        let attachment: serenity::all::Attachment = serde_json::from_value(json!({"id":"7","filename":"hub.lua","size":size,"url":"https://cdn.discordapp.com/script.lua","proxy_url":"https://media.discordapp.net/script.lua"})).unwrap();
        command
            .data
            .resolved
            .attachments
            .insert(attachment.id, attachment);
        command.data.options = serde_json::from_value(json!([{"name":"file","type":11,"value":"7"},{"name":"private","type":5,"value":false}])).unwrap();
        let (http, calls, task) = discord_server(vec![(204, String::new())]).await;
        h.from_interaction(http, command).await.unwrap();
        task.await.unwrap();
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        let callback = json_body(&calls[0]);
        assert_eq!(callback["type"], 4);
        assert_eq!(callback["data"]["flags"], 64);
        assert!(callback["data"]["content"]
            .as_str()
            .unwrap()
            .contains(needle));
        assert!(callback["data"]["attachments"]
            .as_array()
            .is_none_or(Vec::is_empty));
        assert!(backend.seen.lock().unwrap().is_empty());
        assert!(net.calls.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn downloaded_names_are_bounded_and_failed_names_downloads_never_reach_backend() {
    for failed in [false, true] {
        let backend = Arc::new(FakeBackend::default());
        let net = Arc::new(FakeNetwork {
            names: vec![b' '; bot::NAMES_MAX_BYTES + 1],
            names_error: failed.then_some("http"),
            ..Default::default()
        });
        let h = make(config(), backend.clone(), net.clone());
        let mut responder = Recorder::default();
        h.run(
            &mut responder,
            Request {
                file: Some(file("hub.lua")),
                names_file: Some(InputFile {
                    filename: "names.json".into(),
                    size: 10,
                    url: "https://cdn.discordapp.com/names.json".into(),
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(responder.accepted);
        let (delivery, card) = responder.final_.unwrap();
        assert!(!delivery.ok && delivery.files.is_empty());
        assert_eq!(
            delivery.text,
            if failed {
                "Failed to download the attachment from Discord. Try sending it again."
            } else {
                "The names file is too large (max 256 KB)."
            }
        );
        assert!(card.unwrap().title.starts_with('❌'));
        assert!(backend.seen.lock().unwrap().is_empty());
        assert_eq!(
            *net.calls.lock().unwrap(),
            [
                (
                    "https://cdn.discordapp.com/script.lua".into(),
                    3 * 1024 * 1024,
                    vec![],
                    Duration::from_secs(25)
                ),
                (
                    "https://cdn.discordapp.com/names.json".into(),
                    256 * 1024,
                    vec![],
                    Duration::from_secs(25)
                ),
            ]
        );
    }
}

#[tokio::test]
async fn real_chat_sandbox_url_forwards_options_and_delivers_only_trace_files() {
    let mut cfg = config();
    cfg.progress_card = false;
    let backend = Arc::new(FakeBackend {
        silent: true,
        ..Default::default()
    });
    let net = Arc::new(FakeNetwork::default());
    let h = make(cfg, backend.clone(), net.clone());
    let mut message = wire_message();
    message.content = "!uwu sandbox https://github.com/u/r/blob/main/hub.lua".into();
    let (http, calls, task) = discord_server(vec![(200, wire_reply()), (200, wire_reply())]).await;
    h.from_chat(http, message, bot::DM_UPLOAD_LIMIT, true)
        .await
        .unwrap();
    task.await.unwrap();
    {
        let seen = backend.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, b"-- protected");
        let opts = &seen[0].1;
        assert!(opts.sandbox_only);
        assert!(!opts.trace && !opts.moonveil && !opts.annotate && opts.names.is_none());
        assert_eq!(opts.max_bytes, 3 * 1024 * 1024);
    }
    assert_eq!(
        net.calls.lock().unwrap()[0].0,
        "https://raw.githubusercontent.com/u/r/main/hub.lua"
    );
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    let payload = json_body(&calls[1]);
    assert_eq!(
        payload["attachments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["filename"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["hub_trace.txt", "hub_payloads.zip"]
    );
    let content = payload["content"].as_str().unwrap();
    assert!(content.contains("nothing was deobfuscated"));
    assert!(content.contains("It carries a Bacon Guard v0.42.0 banner"));
    assert!(content.contains("the log shows the protected script itself"));
    assert!(!content.contains("Detected") && !content.contains("Bytecode"));
    assert!(calls[1].contains("FAKE ROBLOX SANDBOX TRACE  -  hub\nsaved as hub_payload1.lua\n"));
}

#[tokio::test]
async fn real_chat_delivers_assumed_and_automatically_detected_moonveil_without_trace() {
    for (content, forced, assumed, filename) in [
        ("!moonveil", true, true, "My_Hub_reconstructed.lua"),
        ("!uwu", false, false, "My_Hub_reconstructed.lua"),
        (
            "!moonveil https://x.io/from_url.lua",
            true,
            true,
            "from_url_reconstructed.lua",
        ),
    ] {
        let mut cfg = config();
        cfg.progress_card = false;
        let reconstruction =
            "-- Behaviour reconstruction, NOT the original source\nprint(\"observed\")\n";
        let backend = Arc::new(FakeBackend {
            silent: true,
            result: Some(Output {
                text: Some(reconstruction.into()),
                info: json!({"kind":"moonveil","trace_only":true,"moonveil":{"version":"2.1.1","assumed":assumed},"calls":120,"threads":5,"lines":40,"syntax_ok":true}),
                trace: None,
                raw: None,
            }),
            ..Default::default()
        });
        let net = Arc::new(FakeNetwork::default());
        let h = make(cfg, backend.clone(), net.clone());
        let mut message = wire_message();
        message.content = content.into();
        let from_url = content.contains("https://");
        let mut responses = vec![(200, wire_reply())];
        if from_url {
            responses.push((200, wire_reply()));
        } else {
            message.attachments = serde_json::from_value(json!([{"id":"7","filename":"My Hub.lua","size":100,"url":"https://cdn.discordapp.com/script.lua","proxy_url":"https://media.discordapp.net/script.lua"}])).unwrap();
        }
        let (http, calls, task) = discord_server(responses).await;
        h.from_chat(http, message, bot::DM_UPLOAD_LIMIT, true)
            .await
            .unwrap();
        task.await.unwrap();
        {
            let seen = backend.seen.lock().unwrap();
            assert_eq!(seen.len(), 1);
            assert_eq!(seen[0].1.moonveil, forced);
            assert!(!seen[0].1.trace && !seen[0].1.sandbox_only && !seen[0].1.annotate);
            assert!(seen[0].1.names.is_none());
        }
        assert_eq!(net.calls.lock().unwrap().len(), 1);
        if from_url {
            assert_eq!(net.calls.lock().unwrap()[0].0, "https://x.io/from_url.lua");
        }
        let calls = calls.lock().unwrap();
        let last = calls.last().unwrap();
        let payload = json_body(last);
        assert_eq!(payload["attachments"].as_array().unwrap().len(), 1);
        assert_eq!(payload["attachments"][0]["filename"], filename);
        assert!(last.contains(reconstruction));
        let text = payload["content"].as_str().unwrap();
        assert!(text.starts_with("✅ Done"));
        for phrase in [
            "NOT the original source",
            "allowed to analyze",
            "120 calls seen in 5 threads/callbacks, 40 output lines",
        ] {
            assert!(text.contains(phrase), "{content}: {text}");
        }
        assert_eq!(text.contains("No MoonVeil banner found"), assumed);
        assert_eq!(text.contains("Detected: MoonVeil 2.1.1"), !assumed);
        assert!(!text.contains("Bytecode") && !text.contains("functions,"));
        assert_eq!(payload["allowed_mentions"]["parse"], json!([]));
    }
}

#[tokio::test]
async fn real_chat_backend_refusals_preserve_moonveil_guidance_and_ordinary_trace_url_advice() {
    for (content, forced, from_url) in [
        ("!moonveil", true, false),
        ("!moonveil https://x.io/hub.lua", true, true),
        ("!uwu trace https://x.io/hub.lua", false, true),
    ] {
        let mut cfg = config();
        cfg.progress_card = false;
        let backend = Arc::new(FakeBackend {
            silent: true,
            error: Some("unsupported"),
            error_detail: forced.then_some("this is a Bacon Guard / FORGE script, not a MoonVeil one; use the normal command for it"),
            ..Default::default()
        });
        let h = make(cfg, backend.clone(), Arc::new(FakeNetwork::default()));
        let mut message = wire_message();
        message.content = content.into();
        let mut responses = vec![(200, wire_reply())];
        if from_url {
            responses.push((200, wire_reply()));
        } else {
            message.attachments = serde_json::from_value(json!([{"id":"7","filename":"guard.lua","size":100,"url":"https://cdn.discordapp.com/script.lua","proxy_url":"https://media.discordapp.net/script.lua"}])).unwrap();
        }
        let (http, calls, task) = discord_server(responses).await;
        h.from_chat(http, message, bot::DM_UPLOAD_LIMIT, true)
            .await
            .unwrap();
        task.await.unwrap();
        let calls = calls.lock().unwrap();
        let payload = json_body(calls.last().unwrap());
        let text = payload["content"].as_str().unwrap();
        assert!(text.contains("won't guess"));
        assert!(payload["attachments"].as_array().is_none_or(Vec::is_empty));
        assert!(!text.contains("/srv/secret"));
        if forced {
            assert!(
                text.contains("Bacon Guard / FORGE script")
                    && text.contains("use the normal command")
            );
            assert!(!text.contains("Downloaded the file") && !text.contains("Check the raw link"));
        } else {
            assert!(text.contains("unknown operation 77"));
            assert!(text.contains("Downloaded the file, but couldn't deobfuscate it"));
            assert!(text.contains("Check the raw link or attach the original"));
        }
        let seen = backend.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].1.moonveil, forced);
        assert_eq!(seen[0].1.trace, !forced);
    }
}

#[test]
fn delivered_static_report_cleanup_and_syntax_warning_keep_their_scope_and_scrub_paths() {
    let mut result = output();
    result.info["unpacking"] = json!("static");
    result.info["source_sha256"] =
        json!("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef");
    result.info["cleaned"] = json!({"strings":2,"numbers":3,"checks":4,"arms":5});
    result.info["raw_lines"] = json!(170);
    result.info["verification"]["raw_applies_to_delivery"] = json!(false);
    result.info["warnings"] =
        json!(["the result has a syntax error: /srv/bots/secret/tmp123.lua(8,14): SyntaxError: x"]);
    let delivery = bot::package("static", &result, 1.0, bot::DM_UPLOAD_LIMIT).unwrap();
    assert_eq!(delivery.files[0].1, b"print(\"hi\")\n");
    assert_eq!(delivery.files[1].0, "static_verification.txt");
    let report = std::str::from_utf8(&delivery.files[1].1).unwrap();
    assert!(report.contains("Normalized source SHA-256: 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\nUnpacking: bounded static image decoding; loader and application not run.\n"));
    assert!(report.contains("does not cover the cleaned file"));
    assert!(delivery.text.contains("2 encrypted strings decrypted, 3 number puzzles solved, 4 always-true / always-false checks removed (5 branches that could never run); the raw decompile had 170 lines"));
    assert!(delivery
        .text
        .contains("Bytecode check of the raw decompile"));
    assert!(delivery
        .text
        .contains("Note: the result has a syntax error: line 8:14: SyntaxError: x"));
    assert!(!delivery.text.contains("/srv/") && !delivery.text.contains("secret"));
    result.info["cleaned"] = json!({"statements":1});
    assert!(bot::package("static", &result, 1.0, bot::DM_UPLOAD_LIMIT)
        .unwrap()
        .text
        .contains("nothing to fold; the raw decompile had 170 lines"));
}

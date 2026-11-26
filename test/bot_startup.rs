use std::time::Duration;

#[tokio::test]
async fn real_bot_exits_one_for_missing_token_and_invalid_config_before_connecting() {
    for (invalid_config, expected) in [
        (false, "DISCORD_TOKEN is not set."),
        (true, "Invalid configuration: MAX_MB (0-8)"),
    ] {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
        let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_bgdeobf-bot"));
        command
            .current_dir(directory.path())
            .env_clear()
            .kill_on_drop(true);
        if invalid_config {
            command.env("MAX_MB", "99");
        }
        let output = tokio::time::timeout(Duration::from_secs(10), command.output())
            .await
            .expect("invalid startup did not exit before the deadline")
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.starts_with(expected), "{stderr}");
        assert_eq!(stderr.lines().count(), 1, "{stderr}");
        assert!(!stderr.contains("Problem with a Luau tool"));
        assert!(!stderr.contains("Couldn't connect to Discord"));
        if invalid_config {
            assert!(!stderr.contains("DISCORD_TOKEN is not set"));
        }
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }
}

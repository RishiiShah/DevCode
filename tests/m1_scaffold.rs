use clap::Parser;
use devcode::cli::{Cli, Command, Profile};
use devcode::config::{
    default_test_globs, load_config, parse_config, Config, DEFAULT_TIMEOUT_SECONDS,
};
use std::process::Command as ProcessCommand;

#[test]
fn test_cli_version_flag_via_binary() {
    let binary = env!("CARGO_BIN_EXE_devcode");
    let output = ProcessCommand::new(binary)
        .arg("--version")
        .output()
        .expect("failed to execute binary");

    assert!(output.status.success(), "devcode --version should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("devcode") && stdout.contains("0.1.0"),
        "stdout should contain devcode and version 0.1.0, got: {stdout}"
    );
}

#[test]
fn test_cli_unknown_flag_exits_2() {
    let binary = env!("CARGO_BIN_EXE_devcode");
    let output = ProcessCommand::new(binary)
        .arg("--unknown-flag")
        .output()
        .expect("failed to execute binary");

    assert_eq!(
        output.status.code(),
        Some(2),
        "unknown flag should exit with code 2"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unexpected argument") || stderr.contains("unknown"),
        "stderr should mention unexpected argument, got: {stderr}"
    );
}

#[test]
fn test_cli_help_flag_exits_0() {
    let binary = env!("CARGO_BIN_EXE_devcode");
    let output = ProcessCommand::new(binary)
        .arg("--help")
        .output()
        .expect("failed to execute binary");

    assert!(output.status.success(), "devcode --help should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage:"));
}

#[test]
fn test_cli_missing_configuration_and_manifest() {
    let binary = env!("CARGO_BIN_EXE_devcode");

    let root = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    // Root execution
    let output_root = ProcessCommand::new(binary)
        .arg("prompt")
        .current_dir(root.path())
        .env("DEVCODE_CONFIG_DIR", config.path())
        .env_remove("NVIDIA_API_KEY")
        .output()
        .expect("failed to execute binary");
    assert_eq!(output_root.status.code(), Some(1));
    let stderr_root = String::from_utf8_lossy(&output_root.stderr);
    assert!(stderr_root.contains("devcode auth login"));

    // Rollback subcommand
    let output_rollback = ProcessCommand::new(binary)
        .arg("rollback")
        .current_dir(root.path())
        .output()
        .expect("failed to execute binary");
    assert_eq!(output_rollback.status.code(), Some(1));
    let stderr_rollback = String::from_utf8_lossy(&output_rollback.stderr);
    assert!(stderr_rollback.contains("no manifest found"));

    // Rollback with step
    let output_rollback_step = ProcessCommand::new(binary)
        .args(["rollback", "--step", "3"])
        .current_dir(root.path())
        .output()
        .expect("failed to execute binary");
    assert_eq!(output_rollback_step.status.code(), Some(1));
    let stderr_rollback_step = String::from_utf8_lossy(&output_rollback_step.stderr);
    assert!(stderr_rollback_step.contains("no manifest found"));
}

#[test]
fn test_cli_parsing_structure() {
    // Default profile is build
    let cli = Cli::try_parse_from(["devcode", "do something"]).unwrap();
    assert_eq!(cli.profile, Profile::Build);
    assert_eq!(cli.prompt.as_deref(), Some("do something"));
    assert_eq!(cli.command, None);

    // Profile ask
    let cli = Cli::try_parse_from(["devcode", "--profile", "ask", "explain this"]).unwrap();
    assert_eq!(cli.profile, Profile::Ask);
    assert_eq!(cli.prompt.as_deref(), Some("explain this"));
    assert_eq!(cli.command, None);

    // Profile plan
    let cli = Cli::try_parse_from(["devcode", "--profile", "plan"]).unwrap();
    assert_eq!(cli.profile, Profile::Plan);
    assert_eq!(cli.prompt, None);
    assert_eq!(cli.command, None);

    // Rollback subcommand
    let cli = Cli::try_parse_from(["devcode", "rollback", "--step", "5"]).unwrap();
    assert_eq!(cli.command, Some(Command::Rollback { step: Some(5) }));
}

#[test]
fn test_config_missing_file_gives_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = load_config(dir.path()).expect("missing config should return defaults");
    assert_eq!(cfg, Config::default());
    assert_eq!(cfg.sandbox.timeout_seconds, DEFAULT_TIMEOUT_SECONDS);
    assert_eq!(cfg.verification.test_command, None);
    assert_eq!(cfg.verification.test_globs, default_test_globs());
}

#[test]
fn test_config_valid_file() {
    let dir = tempfile::tempdir().unwrap();
    let json_content = r#"{
        "verification": {
            "test_command": "cargo test",
            "test_globs": ["custom_tests/**/*.rs"]
        },
        "sandbox": {
            "timeout_seconds": 60
        }
    }"#;
    std::fs::write(dir.path().join(".devcode.json"), json_content).unwrap();
    let cfg = load_config(dir.path()).expect("valid config should parse");
    assert_eq!(cfg.verification.test_command.as_deref(), Some("cargo test"));
    assert_eq!(cfg.verification.test_globs, vec!["custom_tests/**/*.rs"]);
    assert_eq!(cfg.sandbox.timeout_seconds, 60);
}

#[test]
fn test_config_unknown_field_names_the_field() {
    // Root level unknown field
    let json_root = r#"{
        "unexpected_field": "val"
    }"#;
    let err_root = parse_config(json_root).unwrap_err();
    let msg_root = err_root.to_string();
    assert!(
        msg_root.contains("unexpected_field"),
        "error should name unexpected_field: {msg_root}"
    );

    // Verification level unknown field
    let json_verif = r#"{
        "verification": {
            "unknown_verif_prop": 123
        }
    }"#;
    let err_verif = parse_config(json_verif).unwrap_err();
    let msg_verif = err_verif.to_string();
    assert!(
        msg_verif.contains("unknown_verif_prop"),
        "error should name unknown_verif_prop: {msg_verif}"
    );

    // Sandbox level unknown field
    let json_sandbox = r#"{
        "sandbox": {
            "extra_sandbox_option": true
        }
    }"#;
    let err_sandbox = parse_config(json_sandbox).unwrap_err();
    let msg_sandbox = err_sandbox.to_string();
    assert!(
        msg_sandbox.contains("extra_sandbox_option"),
        "error should name extra_sandbox_option: {msg_sandbox}"
    );
}

#[test]
fn test_config_wrong_type() {
    let json_wrong_type = r#"{
        "sandbox": {
            "timeout_seconds": "not-an-integer"
        }
    }"#;
    let err = parse_config(json_wrong_type).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("invalid type"),
        "error should indicate invalid type: {msg}"
    );
}

use devcode::config::Config;
use devcode::gate::{
    append_skip_tests_audit, evaluate_gate, verify_audit_chain, ChangeClass, CheckStatus,
    GateError, ToolLog,
};
use std::fs;
use std::path::PathBuf;

#[test]
fn test_source_edit_no_test_change_blocked() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    let mut cfg = Config::default();
    cfg.verification.test_command = Some("cargo test".to_string());

    let mut tool_log = ToolLog::new();
    tool_log.record_file_edit("src/lib.rs", 100);
    tool_log.record_command("cargo test", Some(0), 200);

    // Only source modified, no test files
    let diff = vec![(
        PathBuf::from("src/lib.rs"),
        "fn old() {}".to_string(),
        "fn new() {}".to_string(),
    )];

    let res = evaluate_gate(root, &cfg, &tool_log, &diff, false, None, "diff_sha_1");
    assert!(
        matches!(res, Err(GateError::MissingTestAssertion(_))),
        "source edit without test change must be blocked, got: {:?}",
        res
    );
}

#[test]
fn test_source_plus_test_edit_and_passing_command_allowed() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    let mut cfg = Config::default();
    cfg.verification.test_command = Some("cargo test".to_string());

    let mut tool_log = ToolLog::new();
    tool_log.record_file_edit("src/lib.rs", 100);
    tool_log.record_file_edit("tests/test_lib.rs", 150);
    tool_log.record_command("cargo test", Some(0), 200); // Ran AFTER last edit

    let diff = vec![
        (
            PathBuf::from("src/lib.rs"),
            "fn old() {}".to_string(),
            "fn new() {}".to_string(),
        ),
        (
            PathBuf::from("tests/test_lib.rs"),
            "fn test_old() {}".to_string(),
            "fn test_new() {}".to_string(),
        ),
    ];

    let summary = evaluate_gate(root, &cfg, &tool_log, &diff, false, None, "diff_sha_2")
        .expect("should be allowed");

    assert_eq!(summary.status, CheckStatus::Fresh);
    assert_eq!(summary.classification, ChangeClass::ClassA);
    assert_eq!(
        summary.test_files_touched,
        vec![PathBuf::from("tests/test_lib.rs")]
    );
}

#[test]
fn test_edit_after_passing_run_blocked_as_stale() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    let mut cfg = Config::default();
    cfg.verification.test_command = Some("cargo test".to_string());

    let mut tool_log = ToolLog::new();
    tool_log.record_command("cargo test", Some(0), 100);
    tool_log.record_file_edit("src/lib.rs", 200); // Edit AFTER test run!

    let diff = vec![
        (
            PathBuf::from("src/lib.rs"),
            "fn old() {}".to_string(),
            "fn new() {}".to_string(),
        ),
        (
            PathBuf::from("tests/test_lib.rs"),
            "fn test_old() {}".to_string(),
            "fn test_new() {}".to_string(),
        ),
    ];

    let res = evaluate_gate(root, &cfg, &tool_log, &diff, false, None, "diff_sha_3");
    assert!(
        matches!(res, Err(GateError::TestCommandStale)),
        "edit after test run must be blocked as stale, got: {:?}",
        res
    );
}

#[test]
fn test_test_command_not_run_blocked() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    let mut cfg = Config::default();
    cfg.verification.test_command = Some("cargo test".to_string());

    let mut tool_log = ToolLog::new();
    tool_log.record_file_edit("src/lib.rs", 100);
    tool_log.record_file_edit("tests/test_lib.rs", 150);
    // cargo test was NEVER run

    let diff = vec![
        (
            PathBuf::from("src/lib.rs"),
            "fn old() {}".to_string(),
            "fn new() {}".to_string(),
        ),
        (
            PathBuf::from("tests/test_lib.rs"),
            "fn test_old() {}".to_string(),
            "fn test_new() {}".to_string(),
        ),
    ];

    let res = evaluate_gate(root, &cfg, &tool_log, &diff, false, None, "diff_sha_4");
    assert!(
        matches!(res, Err(GateError::TestCommandNotRun)),
        "not-run test command must be blocked, got: {:?}",
        res
    );
}

#[test]
fn test_whitespace_only_change_is_class_c_and_allowed_without_test() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    let mut cfg = Config::default();
    cfg.verification.test_command = Some("cargo test".to_string());

    let tool_log = ToolLog::new(); // No tests run

    // Whitespace only difference
    let diff = vec![(
        PathBuf::from("src/lib.rs"),
        "fn hello() {\n    let x = 1;\n}\n".to_string(),
        "fn hello() {\n        let x = 1;\n}\n".to_string(),
    )];

    let summary = evaluate_gate(root, &cfg, &tool_log, &diff, false, None, "diff_sha_c")
        .expect("Class C whitespace change must be allowed with no test");

    assert_eq!(summary.classification, ChangeClass::ClassC);
}

#[test]
fn test_skip_tests_audit_chain_integrity() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    let files1 = vec![PathBuf::from("src/payment.rs")];
    append_skip_tests_audit(root, "cli", &files1, "sha_diff_1", "hotfix bypass 1").unwrap();

    let files2 = vec![PathBuf::from("src/auth.rs")];
    append_skip_tests_audit(root, "cli", &files2, "sha_diff_2", "hotfix bypass 2").unwrap();

    // Verify valid chain
    verify_audit_chain(root).expect("audit chain should be valid");

    // Tamper with audit log: modify first line
    let audit_path = devcode::state::dir(root).join("audit.log");
    let content = fs::read_to_string(&audit_path).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 2);

    // Tamper with first line's reason
    let tampered_line_0 = lines[0].replace("hotfix bypass 1", "unauthorized edit");
    let tampered_content = format!("{}\n{}\n", tampered_line_0, lines[1]);
    fs::write(&audit_path, tampered_content).unwrap();

    // Verifying chain must now fail!
    let res = verify_audit_chain(root);
    assert!(
        matches!(res, Err(GateError::AuditChainBroken { .. })),
        "tampering with earlier line must break audit chain, got: {:?}",
        res
    );

    // Delete first line
    let deleted_first_line = format!("{}\n", lines[1]);
    fs::write(&audit_path, deleted_first_line).unwrap();

    let res_deleted = verify_audit_chain(root);
    assert!(
        matches!(res_deleted, Err(GateError::AuditChainBroken { .. })),
        "deleting earlier line must break audit chain, got: {:?}",
        res_deleted
    );
}

#[test]
fn test_summary_output_marks_stale_and_not_run_correctly() {
    let mut tool_log = ToolLog::new();
    tool_log.record_command("npm test", Some(0), 100);
    tool_log.record_file_edit("src/index.js", 200);

    let stale_status = tool_log.check_status_for_command("npm test");
    assert_eq!(stale_status, CheckStatus::Stale);

    let not_run_status = tool_log.check_status_for_command("cargo test");
    assert_eq!(not_run_status, CheckStatus::NotRun);

    let mut log2 = ToolLog::new();
    log2.record_file_edit("src/index.js", 100);
    log2.record_command("npm test", Some(0), 200);
    let fresh_status = log2.check_status_for_command("npm test");
    assert_eq!(fresh_status, CheckStatus::Fresh);
}

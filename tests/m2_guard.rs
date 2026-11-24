use devcode::guard::{
    canonicalize, check_write_path, run_bounded, strip_env, BoundedCollector, GuardError,
    DEFAULT_OUT_CAP,
};
use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

#[test]
fn test_symlink_pointing_outside_root_denied() {
    let temp_root = tempfile::tempdir().unwrap();
    let temp_outside = tempfile::tempdir().unwrap();

    let outside_file = temp_outside.path().join("secret.txt");
    fs::write(&outside_file, "secret").unwrap();

    let symlink_path = temp_root.path().join("link_to_outside");
    symlink(temp_outside.path(), &symlink_path).unwrap();

    // Accessing through symlink should be denied
    let result = canonicalize(temp_root.path(), Path::new("link_to_outside/secret.txt"));
    assert!(
        matches!(result, Err(GuardError::OutsideRoot { .. })),
        "symlink pointing outside root must be denied, got: {:?}",
        result
    );

    let write_result = check_write_path(
        temp_root.path(),
        Path::new("link_to_outside/new_secret.txt"),
    );
    assert!(
        matches!(write_result, Err(GuardError::OutsideRoot { .. })),
        "write through symlink pointing outside root must be denied, got: {:?}",
        write_result
    );
}

#[test]
fn test_parent_dir_escape_denied() {
    let temp_root = tempfile::tempdir().unwrap();

    let result = canonicalize(temp_root.path(), Path::new("../escape.txt"));
    assert!(
        matches!(result, Err(GuardError::OutsideRoot { .. })),
        "../ escape must be denied, got: {:?}",
        result
    );

    let result_nested = canonicalize(temp_root.path(), Path::new("subdir/../../escape.txt"));
    assert!(
        matches!(result_nested, Err(GuardError::OutsideRoot { .. })),
        "nested ../ escape must be denied, got: {:?}",
        result_nested
    );
}

#[test]
fn test_devcode_write_denied_case_insensitively() {
    let temp_root = tempfile::tempdir().unwrap();

    // Exact lowercase
    let result_lower = check_write_path(temp_root.path(), Path::new(".devcode/x"));
    assert!(
        matches!(result_lower, Err(GuardError::DevcodeAccessDenied { .. })),
        "write to .devcode/x must be denied"
    );

    // Mixed case .DevCode/x
    let result_mixed = check_write_path(temp_root.path(), Path::new(".DevCode/x"));
    assert!(
        matches!(result_mixed, Err(GuardError::DevcodeAccessDenied { .. })),
        "write to .DevCode/x must be denied"
    );

    // Upper case .DEVCODE/manifests/1.json
    let result_upper = check_write_path(temp_root.path(), Path::new(".DEVCODE/manifests/1.json"));
    assert!(
        matches!(result_upper, Err(GuardError::DevcodeAccessDenied { .. })),
        "write to .DEVCODE/manifests/1.json must be denied"
    );
}

#[test]
fn test_new_file_in_existing_subdirectory_allowed() {
    let temp_root = tempfile::tempdir().unwrap();
    let sub = temp_root.path().join("src").join("module");
    fs::create_dir_all(&sub).unwrap();

    let target = Path::new("src/module/new_file.rs");
    let result = check_write_path(temp_root.path(), target);
    assert!(
        result.is_ok(),
        "new file in existing subdir should be allowed: {:?}",
        result
    );

    let canonical_path = result.unwrap();
    let canonical_root = temp_root.path().canonicalize().unwrap();
    assert!(canonical_path.starts_with(&canonical_root));
    assert!(canonical_path.ends_with("src/module/new_file.rs"));
}

#[test]
fn test_strip_env_removes_patterns_and_keeps_path_home() {
    let vars = vec![
        ("AWS_ACCESS_KEY_ID", "AKIAIOSFODNN7EXAMPLE"),
        (
            "AWS_SECRET_ACCESS_KEY",
            "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        ),
        ("AWS_DEFAULT_REGION", "us-east-1"),
        ("GITHUB_TOKEN", "ghp_1234567890abcdef"),
        ("OPENAI_API_KEY", "sk-12345"),
        ("ANTHROPIC_API_KEY", "ant-12345"),
        ("MY_API_KEY", "key-12345"),
        ("API_KEY", "plain-api-key"),
        ("SLACK_TOKEN", "xoxb-12345"),
        ("TOKEN", "plain-token"),
        ("SSH_AUTH_SOCK", "/tmp/ssh.sock"),
        ("SSH_AGENT_PID", "12345"),
        ("PATH", "/usr/bin:/bin"),
        ("HOME", "/home/developer"),
        ("USER", "developer"),
    ];

    let stripped = strip_env(vars);
    let stripped_keys: Vec<String> = stripped.into_iter().map(|(k, _)| k).collect();

    assert_eq!(
        stripped_keys,
        vec!["PATH".to_string(), "HOME".to_string(), "USER".to_string()]
    );
}

#[test]
fn test_output_cap_truncation() {
    let cap = 1_000;
    let mut collector = BoundedCollector::new(cap);

    // 1 MB of input data
    let head_chunk = b"HEAD_DATA_START";
    collector.push(head_chunk);

    let body = vec![b'X'; 1_000_000];
    collector.push(&body);

    let tail_chunk = b"TAIL_DATA_END";
    collector.push(tail_chunk);

    let output = collector.finish();
    assert!(
        output.len() <= cap,
        "output length {} must be <= cap {}",
        output.len(),
        cap
    );

    let output_str = String::from_utf8_lossy(&output);
    assert!(
        output_str.starts_with("HEAD_DATA_START"),
        "truncated output must retain head"
    );
    assert!(
        output_str.ends_with("TAIL_DATA_END"),
        "truncated output must retain tail"
    );
    assert!(
        output_str.contains("... [output truncated:"),
        "truncated output must include truncation marker"
    );
}

#[test]
fn test_run_bounded_1mb_output_enforces_cap() {
    let temp_dir = tempfile::tempdir().unwrap();
    // Generate 1 MB output via python3 or sh
    let res = run_bounded(
        &[
            "python3",
            "-c",
            "import sys; sys.stdout.write('HEAD_MARKER' + 'A' * 1048576 + 'TAIL_MARKER')",
        ],
        temp_dir.path(),
        Duration::from_secs(10),
        DEFAULT_OUT_CAP,
    );

    let res = match res {
        Ok(r) => r,
        Err(_) => {
            // Fallback for environments where python3 is sh
            run_bounded(
                &[
                    "sh",
                    "-c",
                    "printf 'HEAD_MARKER'; awk 'BEGIN { while(i++<50000) print \"AAAAAAAAAAAAAAAAAAAA\" }'; printf 'TAIL_MARKER'",
                ],
                temp_dir.path(),
                Duration::from_secs(10),
                DEFAULT_OUT_CAP,
            )
            .expect("run_bounded with awk/sh should succeed")
        }
    };

    assert!(res.stdout_stderr.len() <= DEFAULT_OUT_CAP);
    let text = res.output_text();
    assert!(text.starts_with("HEAD_MARKER"));
    assert!(text.ends_with("TAIL_MARKER"));
    assert!(text.contains("... [output truncated:"));
}

#[test]
fn test_run_bounded_timeout_kills_process_group() {
    let temp_dir = tempfile::tempdir().unwrap();
    let marker_arg = "devcode_m2_watchdog_unique_marker";

    let t_start = Instant::now();
    let result = run_bounded(
        &[
            "sh",
            "-c",
            &format!("sleep 1000 & sleep 1000 & wait # {}", marker_arg),
        ],
        temp_dir.path(),
        Duration::from_secs(1),
        DEFAULT_OUT_CAP,
    )
    .expect("run_bounded should execute");

    let total_elapsed = t_start.elapsed();
    assert!(result.timed_out, "command should have timed out");

    let time_from_timeout_to_dead = total_elapsed.saturating_sub(Duration::from_secs(1));
    println!(
        "NFR-8 measurement: time from timeout to all-dead: {:?}",
        time_from_timeout_to_dead
    );

    // Check with ps if permissible in the running environment
    if let Ok(output) = Command::new("ps").arg("aux").output() {
        if output.status.success() {
            let ps_out = String::from_utf8_lossy(&output.stdout);
            assert!(
                !ps_out.contains(marker_arg),
                "no process containing the marker should remain running"
            );
        }
    }
}

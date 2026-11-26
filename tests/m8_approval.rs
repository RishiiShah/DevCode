use devcode::approval::{
    apply_guarded_file_edit, check_shell_command, execute_guarded_command, format_diff,
    ApprovalPrompt, CommandCheck,
};
use devcode::cli::{Cli, Command as CliCommand};
use devcode::config::Config;
use devcode::gate::{evaluate_gate, GateError, ToolLog};
use devcode::manifest::rollback_latest;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;

fn init_git_repo(path: &Path) {
    Command::new("git")
        .args(["init"])
        .current_dir(path)
        .output()
        .expect("git init failed");
    Command::new("git")
        .args(["config", "user.name", "DevCode Test"])
        .current_dir(path)
        .output()
        .expect("git config user.name failed");
    Command::new("git")
        .args(["config", "user.email", "test@devcode.test"])
        .current_dir(path)
        .output()
        .expect("git config user.email failed");
}

#[test]
fn test_diff_formatting() {
    let diff = format_diff("a\nb\n", "a\nc\n", "foo.txt");
    assert!(diff.contains("--- a/foo.txt"));
    assert!(diff.contains("+++ b/foo.txt"));
    assert!(diff.contains("-b"));
    assert!(diff.contains("+c"));
}

#[test]
fn timed_out_command_is_not_a_passing_test() {
    let temp = tempfile::tempdir().unwrap();
    let mut prompt = ApprovalPrompt::new(Cursor::new(b""), Vec::new(), true, true);
    let mut log = ToolLog::new();
    let cmd = "sleep 30 &";
    let result =
        execute_guarded_command(temp.path(), cmd, "timeout", 1, 1, &mut prompt, &mut log).unwrap();
    assert!(result.timed_out);
    assert_ne!(
        log.check_status_for_command(cmd),
        devcode::gate::CheckStatus::Fresh
    );
}

#[test]
fn oversized_post_command_file_can_be_rolled_back() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("a"), "original").unwrap();
    let mut prompt = ApprovalPrompt::new(Cursor::new(b""), Vec::new(), true, true);
    let mut log = ToolLog::new();
    execute_guarded_command(
        temp.path(),
        "python3 -c 'open(\"a\",\"wb\").write(b\"x\" * 11000000)'",
        "large",
        1,
        10,
        &mut prompt,
        &mut log,
    )
    .unwrap();
    devcode::manifest::rollback(temp.path(), "large", None).unwrap();
    assert_eq!(fs::read(temp.path().join("a")).unwrap(), b"original");
}

#[test]
fn failed_post_snapshot_preserves_recovery_data() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("a"), "original").unwrap();
    let mut prompt = ApprovalPrompt::new(Cursor::new(b""), Vec::new(), true, true);
    let mut log = ToolLog::new();
    let error = execute_guarded_command(
        temp.path(),
        "python3 -c 'from pathlib import Path; [Path(str(i)).touch() for i in range(5001)]'",
        "recovery",
        1,
        30,
        &mut prompt,
        &mut log,
    )
    .unwrap_err();
    assert!(error.to_string().contains("recovery snapshot retained"));
    let snapshot: devcode::manifest::GuardSetSnapshot = serde_json::from_slice(
        &fs::read(temp.path().join(".devcode/recovery/recovery/1.json")).unwrap(),
    )
    .unwrap();
    let hash = snapshot.files[Path::new("a")].sha256.as_ref().unwrap();
    assert_eq!(
        devcode::manifest::read_blob(temp.path(), hash).unwrap(),
        b"original"
    );
    let recovered = devcode::manifest::recover_snapshot(temp.path(), "recovery", 1).unwrap();
    assert_eq!(fs::read(recovered.join("a")).unwrap(), b"original");
    assert!(devcode::manifest::recover_snapshot(temp.path(), "recovery", 1).is_err());
}

#[test]
fn find_delete_is_reconciled_and_rolled_back() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("victim"), "original").unwrap();
    let mut prompt = ApprovalPrompt::new(Cursor::new(b""), Vec::new(), true, true);
    let mut log = ToolLog::new();
    let result = execute_guarded_command(
        temp.path(),
        "find . -name victim -delete",
        "find",
        1,
        5,
        &mut prompt,
        &mut log,
    )
    .unwrap();
    assert_eq!(
        result.reconciled.unwrap().deleted,
        vec![PathBuf::from("victim")]
    );
    devcode::manifest::rollback(temp.path(), "find", None).unwrap();
    assert_eq!(
        fs::read_to_string(temp.path().join("victim")).unwrap(),
        "original"
    );
}

#[test]
fn shell_symlink_deletion_restores_original_target() {
    let temp = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink("missing-target", temp.path().join("link")).unwrap();
    let mut prompt = ApprovalPrompt::new(Cursor::new(b""), Vec::new(), true, true);
    let mut log = ToolLog::new();
    execute_guarded_command(
        temp.path(),
        "python3 -c 'import os; os.unlink(\"link\")'",
        "link",
        1,
        5,
        &mut prompt,
        &mut log,
    )
    .unwrap();
    devcode::manifest::rollback(temp.path(), "link", None).unwrap();
    assert_eq!(
        fs::read_link(temp.path().join("link")).unwrap(),
        PathBuf::from("missing-target")
    );
}

#[test]
fn test_lexical_guard_table_test_25_cases() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    let guard_set = vec![
        PathBuf::from("src/lib.rs"),
        PathBuf::from("src/main.rs"),
        PathBuf::from("tests/m1_scaffold.rs"),
        PathBuf::from("file.txt"),
    ];

    // Table of >= 25 test cases covering allowed, denied, and ask
    let cases = vec![
        // 1-5: Read-only, safe, and quoted redirection
        ("echo \">\"", "Allowed"),
        ("grep \">\" file.txt", "Allowed"),
        ("ls -la", "Allowed"),
        ("cargo test", "Allowed"),
        ("cargo build > target/build.log", "Allowed"),
        // 6-12: Redirections into .devcode (denied)
        ("echo hi > .devcode/tamper", "Denied"),
        ("echo hi >> .devcode/audit.log", "Denied"),
        ("echo hi > .DevCode/foo", "Denied"),
        ("cmd &> .devcode/log", "Denied"),
        ("cmd >| .devcode/log", "Denied"),
        ("cat file > .devcode/state.json", "Denied"),
        ("cmd 2> .devcode/err.log", "Denied"),
        // 13-16: Redirections into guard set (denied)
        ("echo foo > src/lib.rs", "Denied"),
        ("echo bar >> tests/m1_scaffold.rs", "Denied"),
        ("echo text > file.txt", "Denied"),
        ("echo hello > ./src/lib.rs", "Denied"),
        // 17-21: Heredocs with > inside body (allowed)
        ("cat <<EOF\necho hello > .devcode/test\nEOF", "Allowed"),
        (
            "cat <<EOF1 <<EOF2\n> in body 1\nEOF1\n> in body 2\nEOF2",
            "Allowed",
        ),
        ("cat <<-EOF\n\t> .devcode/bad\nEOF", "Allowed"),
        ("cat <<'EOF'\necho > .devcode/tamper\nEOF", "Allowed"),
        ("cat <<\"EOF\"\necho > src/lib.rs\nEOF", "Allowed"),
        // 22-23: Heredocs with redirection outside body (denied)
        ("cat <<EOF > .devcode/bad\nhello\nEOF", "Denied"),
        (
            "cat <<EOF1 <<EOF2 > .devcode/bad\nbody1\nEOF1\nbody2\nEOF2",
            "Denied",
        ),
        // 24-27: tee, cp, mv targeting protected paths (denied)
        ("echo hi | tee .devcode/bad", "Denied"),
        ("echo hi | tee src/lib.rs", "Denied"),
        ("cp file.txt .devcode/file.txt", "Denied"),
        ("cp file.txt src/lib.rs", "Denied"),
        ("mv file.txt .devcode/file.txt", "Denied"),
        ("mv file.txt src/lib.rs", "Denied"),
        // 28-36: Destructive patterns (ask)
        ("rm -rf target/", "Ask"),
        ("rm file.txt", "Ask"),
        ("mv target/a target/b", "Ask"),
        ("sed -i 's/a/b/' temp.txt", "Ask"),
        ("sed -i.bak 's/a/b/' temp.txt", "Ask"),
        ("git clean -fd", "Ask"),
        ("git checkout -- file.txt", "Ask"),
        ("git restore file.txt", "Ask"),
        ("git reset --hard HEAD", "Ask"),
        ("git stash", "Ask"),
        // 37-39: Additional safe and known gap cases
        ("git status", "Allowed"),
        ("git diff", "Allowed"),
        (
            "python3 -c \"open('src/lib.rs', 'w').write('foo')\"",
            "Allowed",
        ),
    ];

    assert!(
        cases.len() >= 25,
        "Table test must contain at least 25 command strings, got {}",
        cases.len()
    );

    for (cmd, expected) in cases {
        let result = check_shell_command(cmd, root, &guard_set);
        match expected {
            "Allowed" => {
                assert_eq!(
                    result,
                    CommandCheck::Allowed,
                    "Expected Allowed for command: {:?}",
                    cmd
                );
            }
            "Denied" => {
                assert!(
                    matches!(result, CommandCheck::Denied(_)),
                    "Expected Denied for command: {:?}, got: {:?}",
                    cmd,
                    result
                );
            }
            "Ask" => {
                assert!(
                    matches!(result, CommandCheck::Ask(_)),
                    "Expected Ask for command: {:?}, got: {:?}",
                    cmd,
                    result
                );
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn test_approval_flow_piped_stdin_once_always_reject() {
    // 1. "once" approval
    {
        let input = Cursor::new(b"once\n");
        let mut output = Vec::new();
        let mut prompt = ApprovalPrompt::new(input, &mut output, true, false);

        let res = prompt
            .request_edit("file.txt", "old", "new")
            .expect("prompt failed");
        assert!(res, "expected approved once");
        assert!(
            !prompt.always_approved,
            "always_approved should remain false"
        );
    }

    // 2. "always" approval
    {
        let input = Cursor::new(b"always\n");
        let mut output = Vec::new();
        let mut prompt = ApprovalPrompt::new(input, &mut output, true, false);

        let res1 = prompt
            .request_edit("file1.txt", "old", "new")
            .expect("prompt failed");
        assert!(res1, "expected approved");
        assert!(prompt.always_approved, "always_approved should now be true");

        // Second request should not read reader at all
        let res2 = prompt
            .request_edit("file2.txt", "old", "new")
            .expect("prompt failed");
        assert!(res2, "expected auto-approved via always_approved");
    }

    // 3. "reject" approval
    {
        let input = Cursor::new(b"reject\n");
        let mut output = Vec::new();
        let mut prompt = ApprovalPrompt::new(input, &mut output, true, false);

        let res = prompt
            .request_edit("file.txt", "old", "new")
            .expect("prompt failed");
        assert!(!res, "expected rejected");
    }

    // 4. No TTY and no --yes rejects
    {
        let input = Cursor::new(b"");
        let mut output = Vec::new();
        let mut prompt = ApprovalPrompt::new(input, &mut output, false, false);

        let res = prompt
            .request_edit("file.txt", "old", "new")
            .expect("prompt failed");
        assert!(!res, "expected rejected without TTY");
        let out_str = String::from_utf8_lossy(&output);
        assert!(out_str.contains("Edits rejected: no TTY"));
    }

    // 5. --yes auto-approves edits even without TTY
    {
        let input = Cursor::new(b"");
        let mut output = Vec::new();
        let mut prompt = ApprovalPrompt::new(input, &mut output, false, true);

        let res = prompt
            .request_edit("file.txt", "old", "new")
            .expect("prompt failed");
        assert!(res, "expected approved with --yes");
    }

    // 6. Destructive commands always ask, even with --yes
    {
        let input = Cursor::new(b"y\n");
        let mut output = Vec::new();
        let mut prompt = ApprovalPrompt::new(input, &mut output, true, true);

        let res = prompt
            .request_command("rm -rf target/", "destructive pattern")
            .expect("prompt failed");
        assert!(res, "expected approved for destructive command");
        let out_str = String::from_utf8_lossy(&output);
        assert!(out_str.contains("Destructive command detected"));
    }

    // 7. Destructive commands denied with no TTY, even with --yes
    {
        let input = Cursor::new(b"");
        let mut output = Vec::new();
        let mut prompt = ApprovalPrompt::new(input, &mut output, false, true);

        let res = prompt
            .request_command("rm -rf target/", "destructive pattern")
            .expect("prompt failed");
        assert!(!res, "expected denied without TTY");
        let out_str = String::from_utf8_lossy(&output);
        assert!(out_str.contains("Destructive command rejected: no TTY"));
    }
}

#[test]
fn test_scripted_run_reconcile_and_rollback() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    init_git_repo(root);

    let tracked_file = root.join("tracked.txt");
    fs::write(&tracked_file, "original 12345\n").unwrap();

    let test_dir = root.join("tests");
    fs::create_dir_all(&test_dir).unwrap();
    let test_file = test_dir.join("test_app.rs");
    fs::write(&test_file, "// test file\n").unwrap();

    Command::new("git")
        .args(["add", "."])
        .current_dir(root)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(root)
        .output()
        .unwrap();

    let mut cfg = Config::default();
    cfg.verification.test_command = Some("true".to_string());

    let mut tool_log = ToolLog::new();
    // Simulate passing test run before mutations
    tool_log.record_file_edit("tests/test_app.rs", 50);
    tool_log.record_command("true", Some(0), 100);

    let diff = vec![(
        PathBuf::from("tests/test_app.rs"),
        "// test file\n".to_string(),
        "// test file updated\n".to_string(),
    )];

    // Initial gate check passes
    let initial_gate = evaluate_gate(root, &cfg, &tool_log, &diff, false, None, "diff_sha_1");
    assert!(
        initial_gate.is_ok(),
        "initial gate check should pass, got: {:?}",
        initial_gate
    );

    // Run mutation command via python3 -c (which is not blocked lexically)
    let input = Cursor::new(b"");
    let mut output = Vec::new();
    let mut prompt = ApprovalPrompt::new(input, &mut output, true, true);

    let python_cmd = "python3 -c \"open('tracked.txt', 'w').write('tampered by python\\n')\"";
    let cmd_res = execute_guarded_command(
        root,
        python_cmd,
        "test_session_python",
        1,
        30,
        &mut prompt,
        &mut tool_log,
    )
    .expect("command execution failed");

    assert_eq!(cmd_res.exit_code, Some(0));
    let reconciled = cmd_res.reconciled.expect("reconcile should be present");
    assert_eq!(reconciled.modified, vec![PathBuf::from("tracked.txt")]);

    // Verify on-disk file was indeed mutated by python
    let mutated_content = fs::read_to_string(&tracked_file).unwrap();
    assert_eq!(mutated_content, "tampered by python\n");

    // Gate evaluation must now fail with TestCommandStale because file was modified after test
    let post_mutate_gate = evaluate_gate(root, &cfg, &tool_log, &diff, false, None, "diff_sha_2");
    assert!(
        matches!(post_mutate_gate, Err(GateError::TestCommandStale)),
        "gate must be invalidated after shell mutation, got: {:?}",
        post_mutate_gate
    );

    // Now roll back using rollback_latest
    let rollback_rep =
        rollback_latest(root, None).expect("rollback_latest should successfully roll back");
    assert_eq!(
        rollback_rep.restored_paths,
        vec![PathBuf::from("tracked.txt")]
    );

    // Verify file is restored to its exact pre-command state!
    let restored_content = fs::read_to_string(&tracked_file).unwrap();
    assert_eq!(restored_content, "original 12345\n");
}

#[test]
fn test_cli_rollback_subcommand_execution() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    init_git_repo(root);

    let file_path = root.join("tracked.txt");
    fs::write(&file_path, "pre-step content\n").unwrap();

    let mut tool_log = ToolLog::new();
    let input = Cursor::new(b"");
    let mut output = Vec::new();
    let mut prompt = ApprovalPrompt::new(input, &mut output, true, true);

    let python_cmd = "python3 -c \"open('tracked.txt', 'w').write('new content\\n')\"";
    let _ = execute_guarded_command(
        root,
        python_cmd,
        "cli_rollback_session",
        1,
        30,
        &mut prompt,
        &mut tool_log,
    )
    .unwrap();

    assert_eq!(fs::read_to_string(&file_path).unwrap(), "new content\n");

    // Run via devcode::run(cli) simulating `devcode rollback`
    let old_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(root).unwrap();

    let cli = Cli {
        provider: None,
        profile: devcode::Profile::Build,
        yes: false,
        trace: false,
        plain: false,
        command: Some(CliCommand::Rollback { step: None }),
        prompt: None,
    };

    let run_res = devcode::run(cli);
    std::env::set_current_dir(old_cwd).unwrap();

    assert!(run_res.is_ok(), "devcode::run rollback should succeed");
    assert_eq!(
        fs::read_to_string(&file_path).unwrap(),
        "pre-step content\n"
    );
}

#[test]
fn test_guarded_file_edit_workflow() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    init_git_repo(root);

    let rel_path = Path::new("src/code.rs");
    let mut tool_log = ToolLog::new();

    // 1. Rejected edit
    {
        let input = Cursor::new(b"reject\n");
        let mut output = Vec::new();
        let mut prompt = ApprovalPrompt::new(input, &mut output, true, false);

        let res = apply_guarded_file_edit(
            root,
            rel_path,
            "let x = 1;\n",
            "edit_session",
            1,
            &mut prompt,
            &mut tool_log,
        )
        .unwrap();

        assert!(!res, "edit should have been rejected");
        assert!(!root.join(rel_path).exists());
    }

    // 2. Approved edit
    {
        let input = Cursor::new(b"once\n");
        let mut output = Vec::new();
        let mut prompt = ApprovalPrompt::new(input, &mut output, true, false);

        let res = apply_guarded_file_edit(
            root,
            rel_path,
            "let x = 1;\n",
            "edit_session",
            1,
            &mut prompt,
            &mut tool_log,
        )
        .unwrap();

        assert!(res, "edit should have been approved");
        assert_eq!(
            fs::read_to_string(root.join(rel_path)).unwrap(),
            "let x = 1;\n"
        );
    }
}

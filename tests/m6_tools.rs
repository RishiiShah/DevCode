use devcode::tools::{
    glob_paths, grep_search, read_file, truncate_output, ToolError, TOOL_OUTPUT_CAP,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[test]
fn test_gitignore_is_respected() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    fs::write(root.join(".gitignore"), "ignored_dir/\n*.secret\n").unwrap();
    fs::create_dir_all(root.join("ignored_dir")).unwrap();
    fs::write(root.join("ignored_dir").join("file.rs"), "fn ignored() {}").unwrap();
    fs::write(root.join("passwords.secret"), "supersecret").unwrap();
    fs::write(root.join("main.rs"), "fn main() {}").unwrap();

    // Test glob
    let matched_glob = glob_paths(root, "**/*").unwrap();
    assert!(matched_glob.contains(&PathBuf::from("main.rs")));
    assert!(!matched_glob.contains(&PathBuf::from("ignored_dir/file.rs")));
    assert!(!matched_glob.contains(&PathBuf::from("passwords.secret")));

    // Test grep
    let grep_res = grep_search(root, "supersecret", None, None).unwrap();
    assert!(grep_res.contains("No matches found"));
}

#[test]
fn test_path_outside_root_is_denied() {
    let temp_root = tempfile::tempdir().unwrap();
    let temp_outside = tempfile::tempdir().unwrap();
    fs::write(temp_outside.path().join("outside.txt"), "outside data").unwrap();

    // Parent escape via ../
    let res = read_file(temp_root.path(), Path::new("../outside.txt"), None, None);
    assert!(
        matches!(res, Err(ToolError::Guard(_))),
        "reading outside root must be denied via guard"
    );

    // Grep outside root
    let grep_res = grep_search(temp_root.path(), "outside", Some(temp_outside.path()), None);
    assert!(
        matches!(grep_res, Err(ToolError::Guard(_))),
        "grepping outside root must be denied via guard"
    );
}

#[test]
fn test_caps_are_enforced() {
    let huge_content = "A".repeat(100_000);
    let truncated = truncate_output(&huge_content, TOOL_OUTPUT_CAP);
    assert!(
        truncated.len() <= TOOL_OUTPUT_CAP,
        "truncated length {} must be <= cap {}",
        truncated.len(),
        TOOL_OUTPUT_CAP
    );
    assert!(truncated.contains("... [output truncated:"));
}

#[test]
fn test_line_slicing_is_exact_1_based_inclusive() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    let content = "line 1: alpha\nline 2: beta\nline 3: gamma\nline 4: delta\nline 5: epsilon\n";
    fs::write(root.join("test.txt"), content).unwrap();

    // Slice lines 2 to 4 inclusive
    let slice = read_file(root, Path::new("test.txt"), Some(2), Some(4)).unwrap();
    assert!(slice.contains("   2 | line 2: beta"));
    assert!(slice.contains("   3 | line 3: gamma"));
    assert!(slice.contains("   4 | line 4: delta"));
    assert!(!slice.contains("line 1: alpha"));
    assert!(!slice.contains("line 5: epsilon"));

    // Full file read (None, None)
    let full = read_file(root, Path::new("test.txt"), None, None).unwrap();
    assert!(full.contains("   1 | line 1: alpha"));
    assert!(full.contains("   5 | line 5: epsilon"));

    // Invalid range (start > end)
    let err = read_file(root, Path::new("test.txt"), Some(5), Some(2));
    assert!(matches!(err, Err(ToolError::InvalidRange(_))));
}

#[test]
fn test_binary_files_are_reported_not_dumped() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    let mut binary_bytes = vec![0x89, 0x50, 0x4E, 0x47, 0x00, 0x00, 0x00];
    binary_bytes.extend_from_slice(b"fake PNG binary data");
    fs::write(root.join("image.png"), &binary_bytes).unwrap();

    let read_res = read_file(root, Path::new("image.png"), None, None).unwrap();
    assert!(
        read_res.contains("[binary file:"),
        "binary file read must report metadata, got: {}",
        read_res
    );
    assert!(!read_res.contains("fake PNG binary data"));
}

#[test]
fn test_grep_latency_on_1200_files_fixture_warm() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    let sub = root.join("src");
    fs::create_dir_all(&sub).unwrap();

    // Create 1,200 files
    for i in 0..1200 {
        let content = if i == 500 || i == 1000 {
            format!("// file {}\nfn target_symbol_here() {{}}\n", i)
        } else {
            format!("// file {}\nfn standard_function_{}() {{}}\n", i, i)
        };
        fs::write(sub.join(format!("file_{:04}.rs", i)), content).unwrap();
    }

    // Warm-up run
    let _ = grep_search(root, "target_symbol_here", None, None).unwrap();

    // Timed warm run
    let start = Instant::now();
    let res = grep_search(root, "target_symbol_here", None, None).unwrap();
    let duration = start.elapsed();

    assert!(res.contains("target_symbol_here"));
    assert!(res.contains("file_0500.rs"));
    assert!(res.contains("file_1000.rs"));

    println!(
        "CBU §4.1 grep latency on warm 1,200-file fixture: {:?} (target <= 150ms)",
        duration
    );
}

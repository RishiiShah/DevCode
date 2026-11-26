use devcode::agent::{ToolCall, ToolRegistry};
use devcode::approval::ApprovalPrompt;
use devcode::tools::{registry_with_context, MutationContext};
use std::fs;
use std::io::Cursor;
use std::net::TcpListener;
use std::path::Path;

/// Registry with a scripted approval reader: one answer line per approval prompt.
fn registry(root: &Path, answers: &str) -> ToolRegistry {
    registry_with_context(MutationContext::new(
        root.to_path_buf(),
        "managed-test".into(),
        ApprovalPrompt::new(
            Cursor::new(answers.as_bytes().to_vec()),
            Vec::new(),
            true,
            true,
        ),
    ))
}

fn call(tools: &ToolRegistry, name: &str, args: serde_json::Value) -> Result<String, String> {
    tools.execute(&ToolCall {
        id: "t".into(),
        name: name.into(),
        arguments: args.to_string(),
        thought_signature: None,
    })
}

fn start(tools: &ToolRegistry, command: &str, port: u16, ready: u64) -> Result<String, String> {
    call(
        tools,
        "start_process",
        serde_json::json!({"command": command, "port": port, "ready_timeout_secs": ready}),
    )
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn server(port: u16) -> String {
    format!("python3 -m http.server {port} --bind 127.0.0.1")
}

fn port_open(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        std::time::Duration::from_millis(200),
    )
    .is_ok()
}

#[test]
fn startup_failure_reports_exit_and_never_claims_ready() {
    let dir = tempfile::tempdir().unwrap();
    let tools = registry(dir.path(), "y\n");
    let err = start(&tools, "echo boom; exit 3", free_port(), 10).unwrap_err();
    assert!(err.contains("exited before"), "{err}");
    assert!(err.contains("exit code 3"), "{err}");
    assert!(err.contains("boom"), "{err}");
    assert!(!dir
        .path()
        .join(".devcode/managed/proc-1.before.json")
        .exists());
}

#[test]
fn slow_start_keeps_running_in_background_and_can_be_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let tools = registry(dir.path(), "y\n");
    let port = free_port();
    let ok = start(&tools, "sleep 30", port, 1).unwrap();
    assert!(ok.contains("still starting"), "{ok}");
    assert!(ok.contains("keeps running"), "{ok}");
    let listed = call(&tools, "list_processes", serde_json::json!({})).unwrap();
    assert!(listed.contains("proc-1  running"), "{listed}");
    let stopped = call(&tools, "stop_process", serde_json::json!({"id": "proc-1"})).unwrap();
    assert!(stopped.contains("stopped proc-1"), "{stopped}");
    let listed = call(&tools, "list_processes", serde_json::json!({})).unwrap();
    assert!(listed.contains("proc-1  stopped"), "{listed}");
}

#[test]
fn ready_on_ipv6_loopback_and_port_the_server_chose() {
    // The server ignores the requested port, binds ::1 only, and prints its URL (like Vite).
    let dir = tempfile::tempdir().unwrap();
    let tools = registry(dir.path(), "y\n");
    let actual = free_port();
    let command = format!(
        "python3 -c \"import http.server,socket,sys\nclass S(http.server.HTTPServer): address_family = socket.AF_INET6\ns = S(('::1', {actual}), http.server.SimpleHTTPRequestHandler)\nprint('Local: http://localhost:{actual}/', flush=True)\ns.serve_forever()\""
    );
    let ok = start(&tools, &command, free_port(), 10).unwrap();
    assert!(ok.contains(&format!("http://localhost:{actual}")), "{ok}");
    assert!(ok.contains("is ready"), "{ok}");
    assert!(ok.contains("the server chose"), "{ok}");
    call(&tools, "stop_process", serde_json::json!({"id": "proc-1"})).unwrap();
}

#[test]
fn rejected_approval_starts_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let tools = registry(dir.path(), "n\n");
    let err = start(&tools, &server(free_port()), free_port(), 5).unwrap_err();
    assert!(err.contains("rejected"), "{err}");
}

#[test]
fn denied_command_is_not_started() {
    let dir = tempfile::tempdir().unwrap();
    let tools = registry(dir.path(), "y\n");
    let err = start(
        &tools,
        "echo x > .devcode/leak.txt; sleep 30",
        free_port(),
        5,
    )
    .unwrap_err();
    assert!(err.contains("denied"), "{err}");
}

#[test]
fn ready_server_stays_available_until_stop() {
    let dir = tempfile::tempdir().unwrap();
    let tools = registry(dir.path(), "y\n");
    let port = free_port();
    let ok = start(&tools, &server(port), port, 30).unwrap();
    assert!(ok.contains("ready"), "{ok}");
    assert!(port_open(port));

    let status = call(
        &tools,
        "process_status",
        serde_json::json!({"id": "proc-1"}),
    )
    .unwrap();
    assert!(status.contains("running"), "{status}");
    assert!(status.contains("responding"), "{status}");

    let stopped = call(&tools, "stop_process", serde_json::json!({"id": "proc-1"})).unwrap();
    assert!(stopped.contains("stopped"), "{stopped}");
    assert!(!port_open(port));
    assert!(!dir
        .path()
        .join(".devcode/managed/proc-1.before.json")
        .exists());
}

#[test]
fn stop_reconciles_file_changes_made_while_running() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("keep.txt"), "same").unwrap();
    let tools = registry(dir.path(), "y\n");
    let port = free_port();
    let cmd = format!("echo built > made.txt; {}", server(port));
    start(&tools, &cmd, port, 30).unwrap();
    let stopped = call(&tools, "stop_process", serde_json::json!({"id": "proc-1"})).unwrap();
    assert!(stopped.contains("made.txt"), "{stopped}");
    assert!(!stopped.contains("keep.txt"), "{stopped}");
    assert!(
        dir.path().join("made.txt").exists(),
        "reconcile must not roll back"
    );
    assert!(dir.path().join(".devcode/manifests").exists());
}

#[test]
fn status_reports_actual_exit_after_natural_exit() {
    let dir = tempfile::tempdir().unwrap();
    let tools = registry(dir.path(), "y\n");
    let port = free_port();
    // Shell exits after the server is ready; the backgrounded server stays in the group.
    let cmd = format!("{} & sleep 3; exit 7", server(port));
    start(&tools, &cmd, port, 30).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(3500));
    let status = call(
        &tools,
        "process_status",
        serde_json::json!({"id": "proc-1"}),
    )
    .unwrap();
    assert!(status.contains("stopped (exit code 7)"), "{status}");
    assert!(!port_open(port), "group must be killed after leader exit");
}

#[test]
fn server_output_is_visible_and_secrets_stripped() {
    std::env::set_var("DEVCODE_TEST_SECRET_TOKEN", "s3cret-value");
    let dir = tempfile::tempdir().unwrap();
    let tools = registry(dir.path(), "y\n");
    let port = free_port();
    let cmd = format!("echo token=$DEVCODE_TEST_SECRET_TOKEN; {}", server(port));
    start(&tools, &cmd, port, 30).unwrap();
    let status = call(
        &tools,
        "process_status",
        serde_json::json!({"id": "proc-1"}),
    )
    .unwrap();
    assert!(status.contains("token="), "{status}");
    assert!(!status.contains("s3cret-value"), "{status}");
    call(&tools, "stop_process", serde_json::json!({"id": "proc-1"})).unwrap();
}

#[test]
fn dropping_registry_stops_running_servers() {
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    {
        let tools = registry(dir.path(), "y\n");
        start(&tools, &server(port), port, 30).unwrap();
        assert!(port_open(port));
    }
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(!port_open(port), "server survived DevCode exit");
}

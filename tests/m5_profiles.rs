use devcode::agent::{
    run_agent_loop, ExitReason, LoopLimits, Model, ModelError, Request, Response, Role,
    ScriptedModel, Tool, ToolCall, ToolRegistry, Usage,
};
use devcode::profile::{handle_slash_command, Profile};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

struct DummyReadTool;
impl Tool for DummyReadTool {
    fn name(&self) -> &str {
        "read_file"
    }
    fn description(&self) -> &str {
        "Reads a file"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "string" })
    }
    fn execute(&self, _args: &str) -> Result<String, String> {
        Ok("file contents".to_string())
    }
}

struct DummyWriteTool {
    was_called: Arc<AtomicBool>,
}
impl Tool for DummyWriteTool {
    fn name(&self) -> &str {
        "write_file"
    }
    fn description(&self) -> &str {
        "Writes a file"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "string" })
    }
    fn execute(&self, _args: &str) -> Result<String, String> {
        self.was_called.store(true, Ordering::SeqCst);
        Ok("wrote file".to_string())
    }
}

struct InspectingModel {
    captured_requests: Arc<Mutex<Vec<Request>>>,
    inner: ScriptedModel,
}

impl InspectingModel {
    fn new(
        responses: Vec<Result<Response, ModelError>>,
        captured: Arc<Mutex<Vec<Request>>>,
    ) -> Self {
        Self {
            captured_requests: captured,
            inner: ScriptedModel::new(responses),
        }
    }
}

impl Model for InspectingModel {
    fn complete(&self, req: &Request) -> Result<Response, ModelError> {
        self.captured_requests.lock().unwrap().push(req.clone());
        self.inner.complete(req)
    }
}

#[test]
fn test_ask_and_plan_tool_list_contains_no_mutating_tools() {
    let mut tools = ToolRegistry::new();
    tools.register(DummyReadTool);
    tools.register(DummyWriteTool {
        was_called: Arc::new(AtomicBool::new(false)),
    });

    for profile in [Profile::Ask, Profile::Plan] {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let responses = vec![Ok(Response {
            text: Some("No tools needed".to_string()),
            tool_calls: Vec::new(),
            usage: Usage::default(),
        })];

        let model = InspectingModel::new(responses, Arc::clone(&captured));
        let limits = LoopLimits::default();

        let _ = run_agent_loop(profile, &model, &tools, "Hello", &limits).unwrap();

        let reqs = captured.lock().unwrap();
        assert_eq!(reqs.len(), 1);
        let sent_tools = &reqs[0].tools;

        // Tool schemas sent to model in ask/plan must contain read_file and NOT write_file
        let tool_names: Vec<String> = sent_tools.iter().map(|t| t.name.clone()).collect();
        assert!(tool_names.contains(&"read_file".to_string()));
        assert!(
            !tool_names.contains(&"write_file".to_string()),
            "profile {:?} must not send mutating tools in schema: {:?}",
            profile,
            tool_names
        );
    }
}

#[test]
fn test_forced_call_to_write_file_in_ask_returns_error_and_does_not_mutate() {
    let write_called = Arc::new(AtomicBool::new(false));
    let mut tools = ToolRegistry::new();
    tools.register(DummyReadTool);
    tools.register(DummyWriteTool {
        was_called: Arc::clone(&write_called),
    });

    // Model attempts a forced write_file call in ask profile
    let responses = vec![
        Ok(Response {
            text: None,
            tool_calls: vec![ToolCall {
                id: "call_write_force".to_string(),
                name: "write_file".to_string(),
                arguments: "file.txt".to_string(),
            }],
            usage: Usage::default(),
        }),
        Ok(Response {
            text: Some("Acknowledged write failure".to_string()),
            tool_calls: Vec::new(),
            usage: Usage::default(),
        }),
    ];

    let model = ScriptedModel::new(responses);
    let limits = LoopLimits::default();

    let res = run_agent_loop(Profile::Ask, &model, &tools, "Force write", &limits).unwrap();
    assert_eq!(res.exit_reason, ExitReason::Completed);

    // Verify write tool was NOT executed
    assert!(
        !write_called.load(Ordering::SeqCst),
        "write_file execute must not be called in ask profile"
    );

    // Verify error observation was returned to model
    let obs = res
        .messages
        .iter()
        .find(|m| m.role == Role::Tool)
        .expect("must have tool observation");
    assert!(
        obs.content.contains("disabled in 'ask' profile"),
        "error observation must state tool is disabled in ask profile: {}",
        obs.content
    );
}

#[test]
fn test_slash_commands_switch_profiles() {
    let mut profile = Profile::Build;

    // Switch to ask
    let handled = handle_slash_command("/ask", &mut profile);
    assert!(handled);
    assert_eq!(profile, Profile::Ask);

    // Switch to plan
    let handled = handle_slash_command("  /plan  ", &mut profile);
    assert!(handled);
    assert_eq!(profile, Profile::Plan);

    // Switch to build
    let handled = handle_slash_command("/build", &mut profile);
    assert!(handled);
    assert_eq!(profile, Profile::Build);

    // Unknown command
    let handled = handle_slash_command("/unknown", &mut profile);
    assert!(!handled);
    assert_eq!(profile, Profile::Build);
}

#[test]
fn test_plan_prompt_requires_structured_specification() {
    let plan_prompt = Profile::Plan.system_prompt();
    assert!(
        plan_prompt.contains("Goal:"),
        "plan prompt must require Goal"
    );
    assert!(
        plan_prompt.contains("Files:"),
        "plan prompt must require Files"
    );
    assert!(
        plan_prompt.contains("Steps:"),
        "plan prompt must require Steps"
    );
    assert!(
        plan_prompt.contains("Risks:"),
        "plan prompt must require Risks"
    );
    assert!(
        plan_prompt.contains("Proof:"),
        "plan prompt must require Proof"
    );
}

#[test]
fn test_ask_prompt_names_profile_switch() {
    let ask_prompt = Profile::Ask.system_prompt();
    assert!(
        ask_prompt.contains("/build") || ask_prompt.contains("/plan"),
        "ask prompt must name profile to switch to when changes are requested"
    );
}

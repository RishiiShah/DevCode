use devcode::agent::{
    run_agent_loop, ExitReason, LoopLimits, ModelError, Response, Role, ScriptedModel, Tool,
    ToolCall, ToolRegistry, Usage,
};
use devcode::profile::Profile;
use std::time::Duration;

struct EchoTool;
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }
    fn description(&self) -> &str {
        "Echoes the input back"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "string" })
    }
    fn execute(&self, arguments: &str) -> Result<String, String> {
        Ok(format!("echoed: {}", arguments))
    }
}

struct FailingTool;
impl Tool for FailingTool {
    fn name(&self) -> &str {
        "fail"
    }
    fn description(&self) -> &str {
        "Always fails"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({})
    }
    fn execute(&self, _arguments: &str) -> Result<String, String> {
        Err("intentional failure".to_string())
    }
}

#[test]
fn test_normal_run_ends_on_no_tool_response() {
    let mut tools = ToolRegistry::new();
    tools.register(EchoTool);

    // Step 1: Tool call
    // Step 2: Final response (no tool calls)
    let responses = vec![
        Ok(Response {
            text: Some("Invoking echo tool".to_string()),
            tool_calls: vec![ToolCall {
                id: "call_1".to_string(),
                thought_signature: None,
                name: "echo".to_string(),
                arguments: "hello".to_string(),
            }],
            usage: Usage {
                input_tokens: 100,
                output_tokens: 20,
            },
        }),
        Ok(Response {
            text: Some("Final answer: hello".to_string()),
            tool_calls: Vec::new(),
            usage: Usage {
                input_tokens: 150,
                output_tokens: 30,
            },
        }),
    ];

    let model = ScriptedModel::new(responses);
    let limits = LoopLimits::default();

    let result = run_agent_loop(Profile::Build, &model, &tools, "Please echo hello", &limits)
        .expect("loop should succeed");

    assert_eq!(result.exit_reason, ExitReason::Completed);
    assert_eq!(result.steps, 2);
    assert_eq!(result.total_input_tokens, 250);
    assert_eq!(result.total_output_tokens, 50);

    // Check message log
    let final_msg = result.messages.last().unwrap();
    assert_eq!(final_msg.role, Role::Assistant);
    assert_eq!(final_msg.content, "Final answer: hello");
}

#[test]
fn test_tool_error_becomes_observation_not_crash() {
    let mut tools = ToolRegistry::new();
    tools.register(FailingTool);

    let responses = vec![
        Ok(Response {
            text: None,
            tool_calls: vec![ToolCall {
                id: "call_fail_1".to_string(),
                thought_signature: None,
                name: "fail".to_string(),
                arguments: "{}".to_string(),
            }],
            usage: Usage {
                input_tokens: 50,
                output_tokens: 10,
            },
        }),
        Ok(Response {
            text: Some("Handled failure gracefully".to_string()),
            tool_calls: Vec::new(),
            usage: Usage {
                input_tokens: 80,
                output_tokens: 20,
            },
        }),
    ];

    let model = ScriptedModel::new(responses);
    let limits = LoopLimits::default();

    let result = run_agent_loop(Profile::Build, &model, &tools, "Run failing tool", &limits)
        .expect("loop should not crash on tool error");

    assert_eq!(result.exit_reason, ExitReason::Completed);
    assert_eq!(result.steps, 2);

    // Find the tool observation message
    let tool_obs = result
        .messages
        .iter()
        .find(|m| m.role == Role::Tool)
        .expect("must contain tool observation");
    assert!(
        tool_obs
            .content
            .contains("Error executing tool 'fail': intentional failure"),
        "tool error must be recorded as observation: {}",
        tool_obs.content
    );
}

#[test]
fn test_step_limit_ends_run_with_matching_reason() {
    let tools = ToolRegistry::new();

    // Model keeps making tool calls indefinitely
    let mut responses = Vec::new();
    for i in 0..10 {
        responses.push(Ok(Response {
            text: Some(format!("Step {}", i)),
            tool_calls: vec![ToolCall {
                id: format!("call_{}", i),
                thought_signature: None,
                name: "unknown".to_string(),
                arguments: "".to_string(),
            }],
            usage: Usage {
                input_tokens: 10,
                output_tokens: 5,
            },
        }));
    }

    let model = ScriptedModel::new(responses);
    let limits = LoopLimits {
        max_steps: 3,
        max_wall_time: Duration::from_secs(60),
        max_consecutive_format_errors: 3,
        context_window: None,
        trace: false,
        progress: None,
        stream: true,
        cancel: None,
    };

    let result = run_agent_loop(Profile::Build, &model, &tools, "Prompt", &limits).unwrap();
    assert_eq!(result.exit_reason, ExitReason::StepLimitExceeded);
    assert_eq!(result.steps, 3);
    assert_eq!(result.total_input_tokens, 30);
    assert_eq!(result.total_output_tokens, 15);
}

#[test]
fn test_wall_time_limit_ends_run_with_matching_reason() {
    let tools = ToolRegistry::new();

    let responses = vec![Ok(Response {
        text: Some("First turn".to_string()),
        tool_calls: vec![ToolCall {
            id: "call_1".to_string(),
            thought_signature: None,
            name: "unknown".to_string(),
            arguments: "".to_string(),
        }],
        usage: Usage {
            input_tokens: 10,
            output_tokens: 5,
        },
    })];

    let model = ScriptedModel::new(responses);
    // 0ms timeout so wall time limit trips on the second turn
    let limits = LoopLimits {
        max_steps: 50,
        max_wall_time: Duration::from_millis(0),
        max_consecutive_format_errors: 3,
        context_window: None,
        trace: false,
        progress: None,
        stream: true,
        cancel: None,
    };

    let result = run_agent_loop(Profile::Build, &model, &tools, "Prompt", &limits).unwrap();
    assert_eq!(result.exit_reason, ExitReason::TimeLimitExceeded);
}

#[test]
fn test_consecutive_format_errors_limit_ends_run() {
    let tools = ToolRegistry::new();

    // 3 consecutive format errors
    let responses = vec![
        Err(ModelError::FormatError("malformed json".to_string())),
        Err(ModelError::FormatError("missing closing tag".to_string())),
        Err(ModelError::FormatError("invalid schema".to_string())),
    ];

    let model = ScriptedModel::new(responses);
    let limits = LoopLimits {
        max_steps: 50,
        max_wall_time: Duration::from_secs(60),
        max_consecutive_format_errors: 3,
        context_window: None,
        trace: false,
        progress: None,
        stream: true,
        cancel: None,
    };

    let result = run_agent_loop(Profile::Build, &model, &tools, "Prompt", &limits).unwrap();
    assert_eq!(result.exit_reason, ExitReason::FormatErrorLimitExceeded);
}

#[test]
fn test_token_totals_equal_scripted_usage() {
    let tools = ToolRegistry::new();

    let responses = vec![
        Ok(Response {
            text: None,
            tool_calls: vec![ToolCall {
                id: "c1".to_string(),
                thought_signature: None,
                name: "unknown".to_string(),
                arguments: "".to_string(),
            }],
            usage: Usage {
                input_tokens: 1234,
                output_tokens: 567,
            },
        }),
        Ok(Response {
            text: Some("Done".to_string()),
            tool_calls: Vec::new(),
            usage: Usage {
                input_tokens: 4321,
                output_tokens: 765,
            },
        }),
    ];

    let model = ScriptedModel::new(responses);
    let limits = LoopLimits::default();

    let result = run_agent_loop(Profile::Build, &model, &tools, "Prompt", &limits).unwrap();
    assert_eq!(result.exit_reason, ExitReason::Completed);
    assert_eq!(result.total_input_tokens, 1234 + 4321);
    assert_eq!(result.total_output_tokens, 567 + 765);
}

use std::time::Duration;

use serde_json::{json, Value};

use crate::error::{Error, Result};

const MAX_RETRY_WAIT: u64 = 6;

fn retryable(status: u16) -> bool {
    status == 429 || status >= 500
}

pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

pub struct ModelTurn {
    pub assistant_message: Value,
    pub tool_calls: Vec<ToolCall>,
    pub total_tokens: u64,
}

pub struct OpenAiCompatible {
    endpoint: String,
    api_key: String,
    model: String,
    agent: ureq::Agent,
}

impl OpenAiCompatible {
    pub fn open(home: &std::path::Path) -> Result<Self> {
        let resolved = crate::provider::resolve(home)?;
        let endpoint = chat_endpoint(&resolved.profile.base_url);
        // Replies are not streamed, so a long answer arrives as one slow read.
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(30))
            .timeout_read(Duration::from_secs(600))
            .build();
        Ok(Self {
            endpoint,
            api_key: resolved.api_key,
            model: resolved.profile.model,
            agent,
        })
    }

    pub fn complete(&mut self, messages: &[Value], tools: &[ToolSpec]) -> Result<ModelTurn> {
        let body = json!({
            "model": self.model,
            "messages": messages,
            "tools": tool_payload(tools),
        });
        let mut waited = 0;
        let response = loop {
            let attempt = self
                .agent
                .post(&self.endpoint)
                .set("Authorization", &format!("Bearer {}", self.api_key))
                .send_json(&body);
            match attempt {
                Ok(response) => break response,
                Err(ureq::Error::Status(code, _)) if retryable(code) && waited < MAX_RETRY_WAIT => {
                    waited = waited * 2 + 2;
                    std::thread::sleep(Duration::from_secs(waited));
                }
                Err(ureq::Error::Status(code, response)) => {
                    let body = response.into_string().unwrap_or_default();
                    return Err(Error::new(format!(
                        "model request failed: status {code}: {}",
                        clip_error_body(&body)
                    )));
                }
                Err(ureq::Error::Transport(transport)) => {
                    return Err(Error::new(format!("model request failed: {transport}")));
                }
            }
        };
        let parsed: Value = response
            .into_json()
            .map_err(|error| Error::new(format!("model response was not json: {error}")))?;
        parse_turn(&parsed)
    }
}

// The tool list follows the work mode, so a mode does not offer a tool it will refuse.
pub fn admitted_tools(work_mode: &str) -> Vec<ToolSpec> {
    let reads = matches!(work_mode, "ask" | "plan");
    let mut tools = Vec::new();
    if !reads {
        tools.push(edit_tool());
    }
    tools.push(shell_tool());
    if work_mode == "plan" {
        tools.push(plan_tool());
    }
    if matches!(
        work_mode,
        "ask" | "plan" | "dynamic-workflow" | "multi-task"
    ) {
        tools.push(workflow_tool());
    }
    if !reads {
        tools.push(lesson_tool());
    }
    tools
}

fn edit_tool() -> ToolSpec {
    ToolSpec {
        name: "edit".to_string(),
        description: "Replace one exact occurrence of old with new in a workspace file. Use an empty old string only to create a missing file.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "old": {"type": "string"},
                "new": {"type": "string"}
            },
            "required": ["path", "old", "new"],
            "additionalProperties": false
        }),
    }
}

fn shell_tool() -> ToolSpec {
    ToolSpec {
        name: "shell".to_string(),
        description: "Run one command in the workspace using the operating system shell."
            .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "command": {"type": "string"}
            },
            "required": ["command"],
            "additionalProperties": false
        }),
    }
}

fn plan_tool() -> ToolSpec {
    ToolSpec {
        name: "plan".to_string(),
        description: "Store the plan in the session directory. This does not edit the repo source."
            .to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "text": {"type": "string"}
            },
            "required": ["text"],
            "additionalProperties": false
        }),
    }
}

fn workflow_tool() -> ToolSpec {
    ToolSpec {
        name: "workflow".to_string(),
        description: "Store workflow.json, schema version 1, in the session directory with ops agent, parallel, and pipeline. Storing runs nothing; the kernel runs it only after the user approves the plan.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "script": {"type": "string"}
            },
            "required": ["script"],
            "additionalProperties": false
        }),
    }
}

fn lesson_tool() -> ToolSpec {
    ToolSpec {
        name: "lesson".to_string(),
        description: "Record one lesson for this repo. The kernel keeps it staged and turns it into agent memory only when the completion command of this session exits with code 0.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "symptom": {"type": "string"},
                "rule": {"type": "string"},
                "test": {"type": "string"}
            },
            "required": ["name", "symptom", "rule", "test"],
            "additionalProperties": false
        }),
    }
}

fn clip_error_body(body: &str) -> String {
    let compact: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut clipped: String = compact.chars().take(300).collect();
    if compact.chars().count() > 300 {
        clipped.push_str("...");
    }
    clipped
}

fn chat_endpoint(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.ends_with("/chat/completions") {
        base.to_string()
    } else {
        format!("{base}/chat/completions")
    }
}

fn tool_payload(tools: &[ToolSpec]) -> Vec<Value> {
    tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
                }
            })
        })
        .collect()
}

fn parse_turn(body: &Value) -> Result<ModelTurn> {
    if let Some(message) = body.get("error").and_then(|error| error.get("message")) {
        return Err(Error::new(format!(
            "model error: {}",
            message.as_str().unwrap_or("unknown")
        )));
    }
    let message = body
        .pointer("/choices/0/message")
        .ok_or_else(|| Error::new("model response did not include a message"))?;
    let tool_calls = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|calls| {
            calls
                .iter()
                .map(parse_tool_call)
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?
        .unwrap_or_default();
    let total_tokens = body
        .pointer("/usage/total_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    Ok(ModelTurn {
        assistant_message: message.clone(),
        tool_calls,
        total_tokens,
    })
}

fn parse_tool_call(value: &Value) -> Result<ToolCall> {
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::new("model tool call is missing an id"))?;
    let function = value
        .get("function")
        .ok_or_else(|| Error::new("model tool call is missing a function"))?;
    let name = function
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::new("model tool call is missing a name"))?;
    let arguments = match function.get("arguments") {
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
        None => "{}".to_string(),
    };
    Ok(ToolCall {
        id: id.to_string(),
        name: name.to_string(),
        arguments,
    })
}

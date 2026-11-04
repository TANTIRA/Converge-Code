use std::env;
use std::time::Duration;

use serde_json::{json, Value};

use crate::error::{Error, Result};

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

pub trait Model {
    fn complete(&mut self, messages: &[Value], tools: &[ToolSpec]) -> Result<ModelTurn>;
}

pub struct OpenAiCompatible {
    endpoint: String,
    api_key: String,
    model: String,
    agent: ureq::Agent,
}

impl OpenAiCompatible {
    pub fn from_env() -> Result<Self> {
        let base_url = required("CONVERGE_BASE_URL")?;
        let api_key = required("CONVERGE_API_KEY")?;
        let model = required("CONVERGE_MODEL")?;
        if !(base_url.starts_with("https://") || base_url.starts_with("http://")) {
            return Err(Error::new("CONVERGE_BASE_URL must be an http or https URL"));
        }
        let endpoint = chat_endpoint(&base_url);
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(120))
            .build();
        Ok(Self {
            endpoint,
            api_key,
            model,
            agent,
        })
    }
}

impl Model for OpenAiCompatible {
    fn complete(&mut self, messages: &[Value], tools: &[ToolSpec]) -> Result<ModelTurn> {
        let body = json!({
            "model": self.model,
            "messages": messages,
            "tools": tool_payload(tools),
        });
        let response = self
            .agent
            .post(&self.endpoint)
            .set("Authorization", &format!("Bearer {}", self.api_key))
            .send_json(body)
            .map_err(|error| match error {
                ureq::Error::Status(code, response) => {
                    let body = response.into_string().unwrap_or_default();
                    Error::new(format!(
                        "model request failed: status {code}: {}",
                        clip_error_body(&body)
                    ))
                }
                ureq::Error::Transport(transport) => {
                    Error::new(format!("model request failed: {transport}"))
                }
            })?;
        let parsed: Value = response
            .into_json()
            .map_err(|error| Error::new(format!("model response was not json: {error}")))?;
        parse_turn(&parsed)
    }
}

pub fn admitted_tools() -> Vec<ToolSpec> {
    vec![
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
        },
        ToolSpec {
            name: "shell".to_string(),
            description: "Run one command in the workspace using the operating system shell.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string"}
                },
                "required": ["command"],
                "additionalProperties": false
            }),
        },
    ]
}

pub fn is_admitted(name: &str) -> bool {
    name == "edit" || name == "shell"
}

fn clip_error_body(body: &str) -> String {
    let compact: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut clipped: String = compact.chars().take(300).collect();
    if compact.chars().count() > 300 {
        clipped.push_str("...");
    }
    clipped
}

fn required(name: &str) -> Result<String> {
    match env::var(name) {
        Ok(value) if !value.is_empty() => Ok(value),
        _ => Err(Error::new(format!("{name} is required"))),
    }
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

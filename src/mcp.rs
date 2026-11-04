use std::fs;
use std::path::Path;

use serde::Deserialize;
use serde_json::json;

use crate::error::{Error, Result};
use crate::model::ToolSpec;

#[derive(Deserialize)]
struct Config {
    servers: Vec<Server>,
}

#[derive(Deserialize)]
struct Server {
    name: String,
    tools: Vec<Tool>,
}

#[derive(Deserialize)]
struct Tool {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    read_only: bool,
}

pub fn context_tool_names(home: &Path, task_server: &str, ask: bool) -> Result<Vec<String>> {
    Ok(initial_tools(home, task_server, ask)?
        .into_iter()
        .map(|tool| tool.name)
        .collect())
}

pub fn initial_tools(home: &Path, task_server: &str, ask: bool) -> Result<Vec<ToolSpec>> {
    let path = home.join("mcp.json");
    let bytes =
        fs::read(&path).map_err(|error| Error::new(format!("read {}: {error}", path.display())))?;
    let config: Config = serde_json::from_slice(&bytes)
        .map_err(|error| Error::new(format!("read {}: {error}", path.display())))?;
    let server = config
        .servers
        .into_iter()
        .find(|server| server.name == task_server)
        .ok_or_else(|| Error::new(format!("mcp server {task_server} is not configured")))?;
    let mut tools = Vec::new();
    for tool in server.tools {
        if is_internal(&tool.name) {
            continue;
        }
        if ask && !tool.read_only {
            continue;
        }
        tools.push(ToolSpec {
            name: tool.name,
            description: tool.description,
            parameters: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": true
            }),
        });
    }
    Ok(tools)
}

fn is_internal(name: &str) -> bool {
    matches!(name, "bash" | "shell" | "edit" | "compaction" | "scratch")
}

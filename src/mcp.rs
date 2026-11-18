use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::model::ToolSpec;

const PROTOCOL_VERSION: &str = "2024-11-05";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Deserialize)]
struct Config {
    servers: Vec<Server>,
}

#[derive(Deserialize)]
struct Server {
    name: String,
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
}

// One running MCP server, kept for the life of the process.
struct Client {
    stdin: ChildStdin,
    replies: Receiver<Value>,
    next: u64,
    child: Child,
}

static SERVERS: Mutex<Vec<(String, Client)>> = Mutex::new(Vec::new());
// A tool name belongs to the server that listed it.
static OWNERS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

impl Client {
    fn start(server: &Server) -> Result<Self> {
        let mut child = Command::new(&server.command)
            .args(&server.args)
            .envs(&server.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| Error::new(format!("start mcp server {}: {error}", server.name)))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::new(format!("mcp server {} has no stdin", server.name)))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::new(format!("mcp server {} has no stdout", server.name)))?;
        let (send, replies) = mpsc::channel();
        // The server may log or notify at any time, so one reader keeps the stream ordered.
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines().filter_map(std::result::Result::ok) {
                if let Ok(value) = serde_json::from_str::<Value>(&line) {
                    if send.send(value).is_err() {
                        break;
                    }
                }
            }
        });
        let mut client = Client {
            stdin,
            replies,
            next: 1,
            child,
        };
        client.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "converge", "version": env!("CARGO_PKG_VERSION")},
            }),
        )?;
        client.notify("notifications/initialized", json!({}))?;
        Ok(client)
    }

    fn write_line(&mut self, message: &Value) -> Result<()> {
        writeln!(self.stdin, "{message}")
            .and_then(|_| self.stdin.flush())
            .map_err(|error| Error::new(format!("write to mcp server: {error}")))
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.write_line(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next;
        self.next += 1;
        self.write_line(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))?;
        loop {
            let message = self
                .replies
                .recv_timeout(REQUEST_TIMEOUT)
                .map_err(|_| Error::new(format!("mcp server did not answer {method}")))?;
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                return Err(Error::new(format!("mcp {method} failed: {error}")));
            }
            return Ok(message.get("result").cloned().unwrap_or(Value::Null));
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn context_tool_names(home: &Path, task_server: &str, ask: bool) -> Result<Vec<String>> {
    Ok(initial_tools(home, task_server, ask)?
        .into_iter()
        .map(|tool| tool.name)
        .collect())
}

// Only the server this task uses enters the initial context.
pub fn initial_tools(home: &Path, task_server: &str, ask: bool) -> Result<Vec<ToolSpec>> {
    let config = read_config(home)?;
    let server = config
        .servers
        .into_iter()
        .find(|server| server.name == task_server)
        .ok_or_else(|| Error::new(format!("mcp server {task_server} is not configured")))?;
    let mut servers = SERVERS.lock().expect("mcp");
    let index = match servers.iter().position(|(name, _)| *name == server.name) {
        Some(index) => index,
        None => {
            servers.push((server.name.clone(), Client::start(&server)?));
            servers.len() - 1
        }
    };
    let listed = servers[index].1.request("tools/list", json!({}))?;
    let tools = listed
        .get("tools")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut specs = Vec::new();
    let mut owners = OWNERS.lock().expect("mcp");
    for tool in tools {
        let Some(name) = tool.get("name").and_then(Value::as_str) else {
            continue;
        };
        if is_internal(name) {
            continue;
        }
        let read_only = tool
            .pointer("/annotations/readOnlyHint")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if ask && !read_only {
            continue;
        }
        if !owners.iter().any(|(tool, _)| tool == name) {
            owners.push((name.to_string(), server.name.clone()));
        }
        specs.push(ToolSpec {
            name: name.to_string(),
            description: tool
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            parameters: tool.get("inputSchema").cloned().unwrap_or_else(
                || json!({"type": "object", "properties": {}, "additionalProperties": true}),
            ),
        });
    }
    Ok(specs)
}

// Runs one tool on the server that listed it.
pub fn call(name: &str, arguments: &str) -> Result<String> {
    let owner = OWNERS
        .lock()
        .expect("mcp")
        .iter()
        .find(|(tool, _)| tool == name)
        .map(|(_, server)| server.clone())
        .ok_or_else(|| Error::new(format!("{name} is not a tool of a running mcp server")))?;
    let arguments: Value = serde_json::from_str(arguments).unwrap_or_else(|_| json!({}));
    let mut servers = SERVERS.lock().expect("mcp");
    let client = servers
        .iter_mut()
        .find(|(server, _)| *server == owner)
        .map(|(_, client)| client)
        .ok_or_else(|| Error::new(format!("mcp server {owner} is not running")))?;
    let result = client.request("tools/call", json!({"name": name, "arguments": arguments}))?;
    let failed = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let text = result
        .get("content")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|item| item.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| result.to_string());
    Ok(if failed {
        format!("error: {text}")
    } else {
        text
    })
}

fn read_config(home: &Path) -> Result<Config> {
    let path = home.join("mcp.json");
    let bytes =
        fs::read(&path).map_err(|error| Error::new(format!("read {}: {error}", path.display())))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| Error::new(format!("read {}: {error}", path.display())))
}

fn is_internal(name: &str) -> bool {
    matches!(
        name,
        "bash" | "shell" | "edit" | "plan" | "workflow" | "lesson" | "compaction" | "scratch"
    )
}

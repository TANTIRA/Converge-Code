const fs = require("fs");
const net = require("net");
const os = require("os");
const path = require("path");

const CLIENT_METHODS = [
  "session/attach",
  "session/input",
  "session/selection",
  "permission/respond",
];

const KERNEL_METHODS = [
  "session/stream",
  "session/tool",
  "permission/ask",
  "session/diff",
  "session/status",
];

function convergeHome() {
  const base = process.platform === "win32" ? process.env.USERPROFILE : process.env.HOME;
  return path.join(base || "", ".converge");
}

function endpoint(home, sessionId) {
  if (process.platform === "win32") {
    return "\\\\.\\pipe\\converge-" + sessionId;
  }
  return path.join(home, "run", sessionId + ".sock");
}

function request(method, params, id) {
  return JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n";
}

function withSession(sessionId, params) {
  return Object.assign({ session_id: sessionId }, params || {});
}

function finished(transcript) {
  return transcript.split("\n").some((line) => {
    if (!line.includes('"kind"') || !line.includes("test")) {
      return false;
    }
    return line.includes('"status":0') || line.includes('"status": 0');
  });
}

function findSession(home, workspace) {
  const projects = path.join(home, "projects");
  if (!fs.existsSync(projects)) {
    return null;
  }
  let workspaceReal = workspace;
  try {
    workspaceReal = fs.realpathSync(workspace);
  } catch (error) {
    return null;
  }
  let chosen = null;
  const stack = [projects];
  while (stack.length > 0) {
    const dir = stack.pop();
    let entries = [];
    try {
      entries = fs.readdirSync(dir, { withFileTypes: true });
    } catch (error) {
      continue;
    }
    for (const entry of entries) {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) {
        stack.push(full);
        continue;
      }
      if (entry.name !== "session.json") {
        continue;
      }
      let record;
      try {
        record = JSON.parse(fs.readFileSync(full, "utf8"));
      } catch (error) {
        continue;
      }
      if (!record.session_id || !record.workspace) {
        continue;
      }
      let stored = record.workspace;
      try {
        stored = fs.realpathSync(record.workspace);
      } catch (error) {
        continue;
      }
      if (stored !== workspaceReal) {
        continue;
      }
      const sessionDir = path.dirname(full);
      const transcriptPath = path.join(sessionDir, "transcript.jsonl");
      const transcript = fs.existsSync(transcriptPath)
        ? fs.readFileSync(transcriptPath, "utf8")
        : "";
      if (finished(transcript)) {
        continue;
      }
      const socketPath = endpoint(home, record.session_id);
      if (process.platform !== "win32" && !fs.existsSync(socketPath)) {
        continue;
      }
      const modified = fs.statSync(full).mtimeMs;
      if (!chosen || modified >= chosen.modified) {
        chosen = {
          sessionId: record.session_id,
          socketPath,
          modified,
          transcript,
        };
      }
    }
  }
  return chosen;
}

function connect(socketPath, sessionId) {
  const stream = net.createConnection(socketPath);
  stream.write(request("session/attach", withSession(sessionId, {}), 1));
  return stream;
}

function handleMessage(line, handlers) {
  let message;
  try {
    message = JSON.parse(line);
  } catch (error) {
    return;
  }
  const method = message.method;
  const params = message.params || {};
  if (method === "session/stream") {
    handlers.onStream && handlers.onStream(params.text || "");
  } else if (method === "session/tool") {
    handlers.onTool && handlers.onTool(params.name || "", params.text || "");
  } else if (method === "permission/ask") {
    handlers.onPermission && handlers.onPermission(params.name || "");
  } else if (method === "session/diff") {
    handlers.onDiff && handlers.onDiff(params.diff || "");
  } else if (method === "session/status") {
    handlers.onStatus && handlers.onStatus(params);
  } else if (message.result) {
    handlers.onResult && handlers.onResult(message.result);
  }
}

function activate(context) {
  const vscode = require("vscode");
  const output = vscode.window.createOutputChannel("Converge");
  const bar = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 100);
  bar.text = "Converge";
  bar.show();
  let session = null;
  let stream = null;
  let buffer = "";
  let nextId = 2;

  function write(method, params) {
    if (!stream || !session) {
      return;
    }
    stream.write(request(method, withSession(session.sessionId, params), nextId));
    nextId += 1;
  }

  function show(text) {
    output.appendLine(text);
  }

  function attach() {
    const folder = vscode.workspace.workspaceFolders && vscode.workspace.workspaceFolders[0];
    if (!folder) {
      bar.text = "Converge: no folder";
      return;
    }
    const found = findSession(convergeHome(), folder.uri.fsPath);
    if (!found) {
      bar.text = "Converge: no session";
      return;
    }
    if (stream) {
      stream.destroy();
    }
    session = found;
    buffer = "";
    bar.text = "Converge: " + found.sessionId.slice(0, 8);
    stream = connect(found.socketPath, found.sessionId);
    stream.on("data", (chunk) => {
      buffer += chunk.toString("utf8");
      let index = buffer.indexOf("\n");
      while (index >= 0) {
        const line = buffer.slice(0, index);
        buffer = buffer.slice(index + 1);
        handleMessage(line, {
          onStream: (text) => show(text),
          onTool: (name, text) => show("tool " + name + "\n" + text),
          onPermission: (name) => ask(name),
          onDiff: (diff) => show(diff),
          onStatus: (params) => {
            const mode = [params.work_mode, params.permission_mode].filter(Boolean).join(" ");
            if (mode) {
              bar.text = "Converge: " + mode;
            }
            if (params.text) {
              show(params.text);
            }
          },
          onResult: (result) => {
            if (result.transcript) {
              show(result.transcript);
            }
          },
        });
        index = buffer.indexOf("\n");
      }
    });
    stream.on("error", () => {
      bar.text = "Converge: disconnected";
    });
  }

  function ask(name) {
    vscode.window.showInformationMessage("Allow " + name + "?", "Allow", "Deny").then((choice) => {
      if (!choice) {
        return;
      }
      write("permission/respond", { allow: choice === "Allow" });
    });
  }

  context.subscriptions.push(
    output,
    bar,
    vscode.commands.registerCommand("converge.attach", attach),
    vscode.commands.registerCommand("converge.send", async () => {
      const text = await vscode.window.showInputBox({ prompt: "Converge" });
      if (text) {
        write("session/input", { text });
      }
    }),
    vscode.window.onDidChangeTextEditorSelection((event) => {
      const editor = event.textEditor;
      if (!editor || editor.selection.isEmpty) {
        return;
      }
      write("session/selection", {
        path: editor.document.uri.fsPath,
        text: editor.document.getText(editor.selection),
        start: editor.selection.start.line + 1,
        end: editor.selection.end.line + 1,
      });
    })
  );
  attach();
}

module.exports = {
  CLIENT_METHODS,
  KERNEL_METHODS,
  activate,
  connect,
  convergeHome,
  endpoint,
  findSession,
  handleMessage,
  request,
  withSession,
};

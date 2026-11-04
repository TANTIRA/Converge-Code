const net = require("net");

const CLIENT_METHODS = [
  "session/attach",
  "session/input",
  "session/selection",
  "permission/respond",
];

function request(method, params, id) {
  return JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n";
}

function connect(socketPath, sessionId) {
  const stream = net.createConnection(socketPath);
  stream.write(request("session/attach", { session_id: sessionId }, 1));
  return stream;
}

module.exports = { CLIENT_METHODS, request, connect };

const CLIENT_METHODS = [
  "session/attach",
  "session/input",
  "session/selection",
  "permission/respond",
];

function request(method, params, id) {
  return JSON.stringify({ jsonrpc: "2.0", id, method, params });
}

module.exports = { CLIENT_METHODS, request };

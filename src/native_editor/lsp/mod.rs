//! The native editor's language servers. btmux is their only client: it syncs
//! buffers to them itself and routes each tab's requests (from
//! `@codemirror/lsp-client`, which takes btmux for the server) with ids of
//! its own, answering what the server asks of its client.

pub mod config;
pub mod process;

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::buffer::Applied;
use super::{EditorServerMessage, Inner, SocketId};

/// A server with no buffers stops after this long.
const IDLE: Duration = Duration::from_secs(60);
/// Crashes within a minute after which a server stays down.
const MAX_CRASHES: usize = 3;
/// `id` of the `initialize` request.
const INITIALIZE: i64 = 0;
/// JSON-RPC error codes.
const METHOD_NOT_FOUND: i64 = -32601;
const SERVER_NOT_INITIALIZED: i64 = -32002;
const CONTENT_MODIFIED: i64 = -32801;
const REQUEST_FAILED: i64 = -32803;

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(test, derive(ts_rs::TS))]
pub enum ServerState {
    Starting,
    Ready,
    /// It failed to start, or crashed too often.
    Failed,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[cfg_attr(test, derive(ts_rs::TS))]
pub struct LspServerStatus {
    #[cfg_attr(test, ts(type = "number"))]
    pub id: u64,
    pub name: String,
    pub root: String,
    pub state: ServerState,
    pub message: Option<String>,
    /// Work in progress (`$/progress`), as "title: message (n%)".
    pub progress: Vec<String>,
}

/// A tab's request, by its own id.
enum Pending {
    Tab(SocketId, Value),
}

pub(super) struct Server {
    id: u64,
    name: String,
    root: PathBuf,
    /// Bumped on each restart; messages from an older process are ignored.
    generation: u32,
    handle: Option<process::Handle>,
    state: ServerState,
    message: Option<String>,
    /// The `initialize` result, given to each tab's client.
    init: Option<Value>,
    full_sync: bool,
    save_text: bool,
    buffers: HashSet<Uuid>,
    /// Tabs whose clients connected.
    sockets: HashSet<SocketId>,
    /// Tabs' `initialize` requests waiting for the server's.
    waiting: Vec<(SocketId, Value)>,
    pending: HashMap<i64, Pending>,
    next_request: i64,
    /// The latest diagnostics per URI, for tabs that connect later.
    diagnostics: BTreeMap<String, Value>,
    /// By token: (title, line).
    progress: BTreeMap<String, (String, String)>,
    crashes: VecDeque<Instant>,
    config: crate::config::LanguageServerConfig,
}

#[derive(Default)]
pub(super) struct Lsp {
    servers: HashMap<u64, Server>,
    by_key: HashMap<(String, PathBuf), u64>,
    next_id: u64,
}

fn notification(method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "method": method, "params": params })
}

fn response(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// What btmux supports as a client: `@codemirror/lsp-client`'s features,
/// plus what btmux answers itself.
fn client_capabilities() -> Value {
    json!({
        "general": { "positionEncodings": ["utf-16"], "markdown": { "parser": "marked" } },
        "workspace": {
            "applyEdit": true,
            "workspaceEdit": { "documentChanges": true },
            "configuration": true,
            "workspaceFolders": true,
            "didChangeConfiguration": { "dynamicRegistration": false },
        },
        "textDocument": {
            "synchronization": { "didSave": true, "dynamicRegistration": false },
            "completion": {
                "completionItem": {
                    "snippetSupport": true,
                    "documentationFormat": ["markdown", "plaintext"],
                    "insertReplaceSupport": false,
                },
                "completionList": { "itemDefaults": ["commitCharacters", "editRange", "insertTextFormat"] },
                "completionItemKind": { "valueSet": [] },
                "contextSupport": true,
            },
            "hover": { "contentFormat": ["markdown", "plaintext"] },
            "formatting": {},
            "rename": {},
            "signatureHelp": {
                "contextSupport": true,
                "signatureInformation": {
                    "documentationFormat": ["markdown", "plaintext"],
                    "parameterInformation": { "labelOffsetSupport": true },
                    "activeParameterSupport": true,
                },
            },
            "definition": {},
            "declaration": {},
            "implementation": {},
            "typeDefinition": {},
            "references": {},
            "publishDiagnostics": { "relatedInformation": true, "versionSupport": true },
        },
        "window": { "showMessage": {}, "workDoneProgress": true },
    })
}

fn toml_json(value: &Option<toml::Value>) -> Value {
    value
        .as_ref()
        .and_then(|v| serde_json::to_value(v).ok())
        .unwrap_or(Value::Null)
}

/// `section` (dotted) of the server's settings, for `workspace/configuration`.
fn setting(settings: &Value, section: Option<&str>) -> Value {
    let Some(section) = section.filter(|s| !s.is_empty()) else {
        return settings.clone();
    };
    let mut value = settings;
    for key in section.split('.') {
        match value.get(key) {
            Some(v) => value = v,
            None => return Value::Null,
        }
    }
    value.clone()
}

/// The title and a short line for `$/progress` work; reports have no title.
fn progress_line(value: &Value, title: Option<&str>) -> (String, String) {
    let title = value["title"]
        .as_str()
        .or(title)
        .unwrap_or_default()
        .to_owned();
    let mut line = title.clone();
    if let Some(message) = value["message"].as_str().filter(|m| !m.is_empty()) {
        line = if line.is_empty() {
            message.to_owned()
        } else {
            format!("{line}: {message}")
        };
    }
    if let Some(percent) = value["percentage"].as_u64() {
        line = format!("{line} ({percent}%)");
    }
    (title, line)
}

impl Server {
    fn status(&self) -> LspServerStatus {
        LspServerStatus {
            id: self.id,
            name: self.name.clone(),
            root: crate::fs_ops::client_path(&self.root),
            state: self.state.clone(),
            message: self.message.clone(),
            progress: self
                .progress
                .values()
                .map(|(_, line)| line.clone())
                .collect(),
        }
    }

    fn send(&self, message: Value) {
        if let Some(handle) = &self.handle {
            let _ = handle.tx.send(message);
        }
    }

    fn ready(&self) -> bool {
        self.state == ServerState::Ready
    }
}

impl Inner {
    pub(super) fn lsp_statuses(&self) -> Vec<LspServerStatus> {
        let mut list: Vec<_> = self.lsp.servers.values().map(Server::status).collect();
        list.sort_by_key(|s| s.id);
        list
    }

    fn send_lsp_statuses(&mut self) {
        let msg = super::to_json(&EditorServerMessage::LspStatus {
            servers: self.lsp_statuses(),
        });
        let sockets: Vec<_> = self.sockets.keys().copied().collect();
        for socket in sockets {
            self.send_raw(socket, msg.clone());
        }
    }

    fn send_lsp(&mut self, socket: SocketId, server: u64, message: Value) {
        let msg = super::to_json(&EditorServerMessage::Lsp { server, message });
        self.send_raw(socket, msg);
    }

    /// Give a newly loaded buffer its servers, starting them as needed.
    pub(super) fn lsp_attach(&mut self, buffer: Uuid, target: config::Target) {
        let config = config::current();
        let mut ids = Vec::new();
        for (name, root) in target.servers {
            let Some(server_config) = config.servers.get(&name).cloned() else {
                continue;
            };
            let key = (name.clone(), root.clone());
            let id = match self.lsp.by_key.get(&key) {
                Some(id) => *id,
                None => {
                    let id = self.lsp.next_id;
                    self.lsp.next_id += 1;
                    self.lsp.by_key.insert(key, id);
                    self.lsp.servers.insert(
                        id,
                        Server {
                            id,
                            name,
                            root,
                            generation: 0,
                            handle: None,
                            state: ServerState::Starting,
                            message: None,
                            init: None,
                            full_sync: false,
                            save_text: false,
                            buffers: HashSet::new(),
                            sockets: HashSet::new(),
                            waiting: Vec::new(),
                            pending: HashMap::new(),
                            next_request: 1,
                            diagnostics: BTreeMap::new(),
                            progress: BTreeMap::new(),
                            crashes: VecDeque::new(),
                            config: server_config,
                        },
                    );
                    self.start_server(id, &config.shell);
                    id
                }
            };
            let server = self.lsp.servers.get_mut(&id).expect("just inserted");
            server.buffers.insert(buffer);
            if server.ready() {
                self.did_open(id, buffer);
            }
            ids.push(id);
        }
        if let Some(b) = self.buffers.get_mut(&buffer) {
            b.language_id = Some(target.language_id);
            b.servers = ids;
        }
        self.send_lsp_statuses();
    }

    fn start_server(&mut self, id: u64, shell: &str) {
        let Some(server) = self.lsp.servers.get_mut(&id) else {
            return;
        };
        server.generation += 1;
        let generation = server.generation;
        server.state = ServerState::Starting;
        server.init = None;
        let handle = process::spawn(
            server.name.clone(),
            server.root.clone(),
            server.config.clone(),
            shell.to_owned(),
            move |message| super::hub().lsp_receive(id, generation, message),
            move |exit| super::hub().lsp_exited(id, generation, exit),
        );
        let root = config::uri(&server.root);
        let folder = server
            .root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let _ = handle.tx.send(json!({
            "jsonrpc": "2.0",
            "id": INITIALIZE,
            "method": "initialize",
            "params": {
                "processId": std::process::id(),
                "clientInfo": { "name": "btmux", "version": env!("CARGO_PKG_VERSION") },
                "rootUri": root,
                "rootPath": crate::fs_ops::client_path(&server.root),
                "workspaceFolders": [{ "uri": root, "name": folder }],
                "initializationOptions": toml_json(&server.config.initialization_options),
                "capabilities": client_capabilities(),
            },
        }));
        server.handle = Some(handle);
    }

    fn did_open(&self, server: u64, buffer: Uuid) {
        let (Some(server), Some(b)) = (self.lsp.servers.get(&server), self.buffers.get(&buffer))
        else {
            return;
        };
        server.send(notification(
            "textDocument/didOpen",
            json!({ "textDocument": {
                "uri": config::uri(&b.path),
                "languageId": b.language_id.clone().unwrap_or_default(),
                "version": b.version,
                "text": b.rope.to_string(),
            } }),
        ));
    }

    /// Tell a buffer's servers what changed.
    pub(super) fn lsp_changed(&self, buffer: Uuid, applied: &[Applied]) {
        let Some(b) = self.buffers.get(&buffer) else {
            return;
        };
        if applied.is_empty() {
            return;
        }
        let uri = config::uri(&b.path);
        let mut full = None;
        for id in &b.servers {
            let Some(server) = self.lsp.servers.get(id).filter(|s| s.ready()) else {
                continue;
            };
            if server.full_sync {
                let text = full.get_or_insert_with(|| b.rope.to_string());
                server.send(notification(
                    "textDocument/didChange",
                    json!({
                        "textDocument": { "uri": uri, "version": b.version },
                        "contentChanges": [{ "text": text }],
                    }),
                ));
                continue;
            }
            for change in applied {
                server.send(notification(
                    "textDocument/didChange",
                    json!({
                        "textDocument": { "uri": uri, "version": change.version },
                        "contentChanges": change.changes,
                    }),
                ));
            }
        }
    }

    pub(super) fn lsp_saved(&self, buffer: Uuid) {
        let Some(b) = self.buffers.get(&buffer) else {
            return;
        };
        for id in &b.servers {
            let Some(server) = self.lsp.servers.get(id).filter(|s| s.ready()) else {
                continue;
            };
            let mut params = json!({ "textDocument": { "uri": config::uri(&b.path) } });
            if server.save_text {
                params["text"] = b.rope.to_string().into();
            }
            server.send(notification("textDocument/didSave", params));
        }
    }

    /// The buffer is gone: close it, and stop servers left without buffers.
    pub(super) fn lsp_closed(&mut self, buffer: Uuid, path: &std::path::Path, servers: &[u64]) {
        for id in servers {
            let Some(server) = self.lsp.servers.get_mut(id) else {
                continue;
            };
            server.buffers.remove(&buffer);
            if server.ready() {
                server.send(notification(
                    "textDocument/didClose",
                    json!({ "textDocument": { "uri": config::uri(path) } }),
                ));
            }
            if server.buffers.is_empty() {
                let (id, generation) = (server.id, server.generation);
                tokio::spawn(async move {
                    tokio::time::sleep(IDLE).await;
                    super::hub().lsp_idle(id, generation);
                });
            }
        }
    }

    pub(super) fn lsp_disconnected(&mut self, socket: SocketId) {
        for server in self.lsp.servers.values_mut() {
            server.sockets.remove(&socket);
            server.waiting.retain(|(s, _)| *s != socket);
            server.pending.retain(|_, Pending::Tab(s, _)| *s != socket);
        }
    }

    /// A message from a tab's client to `server`. `version` is the version of
    /// `buffer` its positions refer to.
    pub(super) fn lsp_from_tab(
        &mut self,
        socket: SocketId,
        id: u64,
        mut message: Value,
        buffer: Option<Uuid>,
        version: Option<u64>,
    ) {
        let method = message["method"].as_str().map(str::to_owned);
        let request_id = message.get("id").cloned();
        let Some(server) = self.lsp.servers.get_mut(&id) else {
            if let Some(rid) = request_id {
                self.send_lsp(socket, id, error(rid, REQUEST_FAILED, "No such server"));
            }
            return;
        };
        match (method.as_deref(), request_id) {
            (Some("initialize"), Some(rid)) => {
                server.sockets.insert(socket);
                match (&server.state, &server.init) {
                    (ServerState::Ready, Some(init)) => {
                        let reply = response(rid, init.clone());
                        let diagnostics: Vec<_> = server.diagnostics.values().cloned().collect();
                        self.send_lsp(socket, id, reply);
                        for params in diagnostics {
                            self.send_lsp(
                                socket,
                                id,
                                notification("textDocument/publishDiagnostics", params),
                            );
                        }
                    }
                    (ServerState::Failed, _) => {
                        let message = server.message.clone().unwrap_or_default();
                        self.send_lsp(socket, id, error(rid, REQUEST_FAILED, &message));
                    }
                    _ => server.waiting.push((socket, rid)),
                }
            }
            (Some("shutdown"), Some(rid)) => self.send_lsp(socket, id, response(rid, Value::Null)),
            // btmux syncs documents itself.
            (
                Some(
                    "initialized"
                    | "exit"
                    | "textDocument/didOpen"
                    | "textDocument/didChange"
                    | "textDocument/didClose"
                    | "textDocument/didSave"
                    | "textDocument/willSave",
                ),
                None,
            ) => {}
            (Some("$/cancelRequest"), None) => {
                let cancelled = &message["params"]["id"];
                let found = server
                    .pending
                    .iter()
                    .find_map(|(sid, Pending::Tab(s, rid))| {
                        (*s == socket && rid == cancelled).then_some(*sid)
                    });
                if let Some(sid) = found {
                    server.send(notification("$/cancelRequest", json!({ "id": sid })));
                }
            }
            (Some(_), Some(rid)) => {
                if !server.ready() {
                    let reply = error(rid, SERVER_NOT_INITIALIZED, "The server is starting");
                    self.send_lsp(socket, id, reply);
                    return;
                }
                let current = buffer.and_then(|b| self.buffers.get(&b)).map(|b| b.version);
                if version.is_some() && current != version {
                    let reply = error(rid, CONTENT_MODIFIED, "The document changed");
                    self.send_lsp(socket, id, reply);
                    return;
                }
                let server = self.lsp.servers.get_mut(&id).expect("checked");
                let sid = server.next_request;
                server.next_request += 1;
                server.pending.insert(sid, Pending::Tab(socket, rid));
                message["id"] = sid.into();
                server.send(message);
            }
            (Some(_), None) => server.send(message),
            // Answers to requests btmux handles itself never reach tabs.
            (None, _) => {}
        }
    }
}

impl super::Hub {
    fn lsp_receive(&'static self, id: u64, generation: u32, message: Value) {
        let mut inner = self.inner.lock().unwrap();
        let Some(server) = inner.lsp.servers.get_mut(&id) else {
            return;
        };
        if server.generation != generation {
            return;
        }
        let method = message["method"].as_str().map(str::to_owned);
        match (method, message.get("id").cloned()) {
            (None, Some(rid)) => {
                let Some(sid) = rid.as_i64() else { return };
                if sid == INITIALIZE {
                    inner.lsp_initialized(id, message);
                    return;
                }
                if let Some(Pending::Tab(socket, tab_id)) = server.pending.remove(&sid) {
                    let mut reply = message;
                    reply["id"] = tab_id;
                    inner.send_lsp(socket, id, reply);
                }
            }
            (Some(method), Some(rid)) => {
                let reply = inner.answer_server(id, &method, rid.clone(), &message["params"]);
                match reply {
                    Some(reply) => {
                        if let Some(server) = inner.lsp.servers.get(&id) {
                            server.send(reply);
                        }
                    }
                    // `workspace/applyEdit` loads files first.
                    None => {
                        let edit = message["params"]["edit"].clone();
                        drop(inner);
                        tokio::spawn(async move {
                            let result = self.apply_edit(edit, None).await;
                            let reply = match result {
                                Ok(()) => response(rid, json!({ "applied": true })),
                                Err(e) => {
                                    response(rid, json!({ "applied": false, "failureReason": e }))
                                }
                            };
                            let inner = self.inner.lock().unwrap();
                            if let Some(server) = inner.lsp.servers.get(&id) {
                                server.send(reply);
                            }
                        });
                    }
                }
            }
            (Some(method), None) => inner.server_notification(id, &method, message),
            (None, None) => {}
        }
    }

    fn lsp_exited(&'static self, id: u64, generation: u32, exit: process::Exit) {
        let mut inner = self.inner.lock().unwrap();
        let Some(server) = inner.lsp.servers.get_mut(&id) else {
            return;
        };
        if server.generation != generation {
            return;
        }
        tracing::info!(server = server.name, "{}", exit.message);
        server.handle = None;
        server.progress.clear();
        let now = Instant::now();
        server.crashes.push_back(now);
        while server
            .crashes
            .front()
            .is_some_and(|t| now.duration_since(*t) > Duration::from_secs(60))
        {
            server.crashes.pop_front();
        }
        let pending: Vec<_> = server.pending.drain().collect();
        let waiting = std::mem::take(&mut server.waiting);
        let restart = exit.started && server.crashes.len() < MAX_CRASHES;
        server.message = Some(exit.message.clone());
        server.state = if restart {
            ServerState::Starting
        } else {
            ServerState::Failed
        };
        for (_, Pending::Tab(socket, rid)) in pending {
            inner.send_lsp(socket, id, error(rid, REQUEST_FAILED, &exit.message));
        }
        for (socket, rid) in waiting {
            inner.send_lsp(socket, id, error(rid, REQUEST_FAILED, &exit.message));
        }
        if restart {
            let shell = config::current().shell.clone();
            inner.start_server(id, &shell);
        }
        inner.send_lsp_statuses();
    }

    /// Stop a server that still has no buffers.
    fn lsp_idle(&self, id: u64, generation: u32) {
        let mut inner = self.inner.lock().unwrap();
        let Some(server) = inner.lsp.servers.get(&id) else {
            return;
        };
        if !server.buffers.is_empty() || server.generation != generation {
            return;
        }
        let server = inner.lsp.servers.remove(&id).expect("checked");
        inner
            .lsp
            .by_key
            .remove(&(server.name.clone(), server.root.clone()));
        if let Some(handle) = server.handle {
            let _ = handle
                .tx
                .send(json!({ "jsonrpc": "2.0", "id": -1, "method": "shutdown" }));
            let _ = handle.tx.send(notification("exit", Value::Null));
            drop(handle.stop);
        }
        inner.send_lsp_statuses();
    }
}

impl Inner {
    fn lsp_initialized(&mut self, id: u64, message: Value) {
        let Some(server) = self.lsp.servers.get_mut(&id) else {
            return;
        };
        if let Some(e) = message.get("error") {
            server.state = ServerState::Failed;
            server.message = Some(format!(
                "{} failed to start: {}",
                server.name,
                e["message"].as_str().unwrap_or("unknown error")
            ));
            if let Some(handle) = server.handle.take() {
                drop(handle.stop);
            }
            self.send_lsp_statuses();
            return;
        }
        let result = message["result"].clone();
        let sync = &result["capabilities"]["textDocumentSync"];
        let kind = sync
            .as_u64()
            .or_else(|| sync["change"].as_u64())
            .unwrap_or(1);
        server.full_sync = kind == 1;
        server.save_text = sync["save"]["includeText"].as_bool().unwrap_or(false);
        server.state = ServerState::Ready;
        server.message = None;
        server.init = Some(result.clone());
        server.send(notification("initialized", json!({})));
        let settings = toml_json(&server.config.config);
        if !settings.is_null() {
            server.send(notification(
                "workspace/didChangeConfiguration",
                json!({ "settings": settings }),
            ));
        }
        let buffers: Vec<_> = server.buffers.iter().copied().collect();
        let waiting = std::mem::take(&mut server.waiting);
        for buffer in buffers {
            self.did_open(id, buffer);
        }
        for (socket, rid) in waiting {
            self.send_lsp(socket, id, response(rid, result.clone()));
        }
        self.send_lsp_statuses();
    }

    /// Answer a request the server makes of its client; `None` for
    /// `workspace/applyEdit`, answered once applied.
    fn answer_server(&self, id: u64, method: &str, rid: Value, params: &Value) -> Option<Value> {
        let server = self.lsp.servers.get(&id)?;
        Some(match method {
            "workspace/applyEdit" => return None,
            "workspace/configuration" => {
                let settings = toml_json(&server.config.config);
                let items = params["items"].as_array().cloned().unwrap_or_default();
                let values: Vec<_> = items
                    .iter()
                    .map(|item| setting(&settings, item["section"].as_str()))
                    .collect();
                response(rid, Value::Array(values))
            }
            "workspace/workspaceFolders" => {
                let name = server
                    .root
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                response(
                    rid,
                    json!([{ "uri": config::uri(&server.root), "name": name }]),
                )
            }
            "client/registerCapability"
            | "client/unregisterCapability"
            | "window/workDoneProgress/create"
            | "window/showMessageRequest"
            | "workspace/semanticTokens/refresh"
            | "workspace/inlayHint/refresh"
            | "workspace/codeLens/refresh"
            | "workspace/diagnostic/refresh" => response(rid, Value::Null),
            "window/showDocument" => response(rid, json!({ "success": false })),
            _ => error(rid, METHOD_NOT_FOUND, "Not supported by btmux"),
        })
    }

    fn server_notification(&mut self, id: u64, method: &str, message: Value) {
        let Some(server) = self.lsp.servers.get_mut(&id) else {
            return;
        };
        match method {
            "textDocument/publishDiagnostics" => {
                let params = &message["params"];
                if let Some(uri) = params["uri"].as_str() {
                    if params["diagnostics"]
                        .as_array()
                        .is_some_and(|d| d.is_empty())
                    {
                        server.diagnostics.remove(uri);
                    } else {
                        server.diagnostics.insert(uri.to_owned(), params.clone());
                    }
                }
            }
            "$/progress" => {
                let params = &message["params"];
                let token = params["token"].to_string();
                let value = &params["value"];
                match value["kind"].as_str() {
                    Some("end") => {
                        server.progress.remove(&token);
                    }
                    Some("begin" | "report") => {
                        let title = server.progress.get(&token).map(|(t, _)| t.as_str());
                        let entry = progress_line(value, title);
                        server.progress.insert(token, entry);
                    }
                    _ => {}
                }
                self.send_lsp_statuses();
                return;
            }
            "window/showMessage" | "window/logMessage" | "telemetry/event" => {}
            _ => {}
        }
        let server = self.lsp.servers.get(&id).expect("checked");
        let sockets: Vec<_> = server.sockets.iter().copied().collect();
        for socket in sockets {
            self.send_lsp(socket, id, message.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_by_section() {
        let settings = json!({ "rust-analyzer": { "check": { "command": "clippy" } } });
        assert_eq!(
            setting(&settings, Some("rust-analyzer.check")),
            json!({ "command": "clippy" })
        );
        assert_eq!(setting(&settings, Some("nope")), Value::Null);
        assert_eq!(setting(&settings, None), settings);
    }

    #[test]
    fn progress_lines() {
        let begin = json!({ "kind": "begin", "title": "Indexing", "percentage": 0 });
        let (title, line) = progress_line(&begin, None);
        assert_eq!(line, "Indexing (0%)");
        let report = json!({ "kind": "report", "message": "3/10", "percentage": 30 });
        assert_eq!(
            progress_line(&report, Some(&title)).1,
            "Indexing: 3/10 (30%)"
        );
    }
}

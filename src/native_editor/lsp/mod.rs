//! The native editor's language servers. btmux is their only client: it syncs
//! buffers to them itself and routes each tab's requests (from
//! `@codemirror/lsp-client`, which takes btmux for the server) with ids of
//! its own, answering what the server asks of its client.

pub mod config;
pub mod process;
pub mod watch;

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

enum Pending {
    /// A tab's request, by its own id.
    Tab(SocketId, Value),
    /// btmux's pull of a document's diagnostics.
    Pull(String),
}

/// After a change, wait this long for more before pulling diagnostics.
const PULL_DELAY: Duration = Duration::from_millis(250);

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
    diagnostics: BTreeMap<String, Diagnostics>,
    /// By token: (title, line).
    progress: BTreeMap<String, (String, String)>,
    crashes: VecDeque<Instant>,
    config: crate::config::LanguageServerConfig,
    /// The Multipass instance it runs in, for files in a sandbox.
    sandbox: Option<String>,
    /// It wants diagnostics pulled (`textDocument/diagnostic`).
    pull: bool,
    /// The latest scheduled pull per buffer; older ones are skipped.
    pulls: HashMap<Uuid, u64>,
    next_pull: u64,
    /// `workspace/didChangeWatchedFiles` registrations, by id.
    watchers: Vec<(String, Vec<watch::FileWatcher>)>,
    root_watch: Option<watch::RootWatch>,
    /// `window/showMessageRequest`s waiting for a tab, by token.
    prompts: HashMap<u64, Value>,
    next_prompt: u64,
}

type ServerKey = (String, PathBuf, Option<String>);

/// A document's diagnostics: published, and pulled (`textDocument/diagnostic`).
#[derive(Default)]
struct Diagnostics {
    pushed: Vec<Value>,
    pulled: Vec<Value>,
}

impl Diagnostics {
    fn params(&self, uri: &str) -> Value {
        let all: Vec<_> = self.pushed.iter().chain(&self.pulled).cloned().collect();
        json!({ "uri": uri, "diagnostics": all })
    }
}

#[derive(Default)]
pub(super) struct Lsp {
    servers: HashMap<u64, Server>,
    by_key: HashMap<ServerKey, u64>,
    next_id: u64,
}

/// Semantic token types and modifiers btmux colors.
pub const TOKEN_TYPES: &[&str] = &[
    "namespace",
    "type",
    "class",
    "enum",
    "interface",
    "struct",
    "typeParameter",
    "parameter",
    "variable",
    "property",
    "enumMember",
    "event",
    "function",
    "method",
    "macro",
    "keyword",
    "modifier",
    "comment",
    "string",
    "number",
    "regexp",
    "operator",
    "decorator",
];
pub const TOKEN_MODIFIERS: &[&str] = &[
    "declaration",
    "definition",
    "readonly",
    "static",
    "deprecated",
    "abstract",
    "async",
    "modification",
    "documentation",
    "defaultLibrary",
];

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
            "didChangeWatchedFiles": { "dynamicRegistration": true, "relativePatternSupport": true },
            "executeCommand": {},
            "symbol": { "resolveSupport": { "properties": ["location.range"] } },
            "semanticTokens": { "refreshSupport": true },
            "inlayHint": { "refreshSupport": true },
            "codeLens": { "refreshSupport": true },
            "diagnostics": { "refreshSupport": true },
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
            "diagnostic": { "relatedDocumentSupport": false },
            "codeAction": {
                "codeActionLiteralSupport": { "codeActionKind": { "valueSet": [
                    "", "quickfix", "refactor", "refactor.extract", "refactor.inline",
                    "refactor.rewrite", "source", "source.organizeImports", "source.fixAll",
                ] } },
                "isPreferredSupport": true,
                "disabledSupport": true,
                "dataSupport": true,
                "resolveSupport": { "properties": ["edit"] },
            },
            "codeLens": {},
            "documentHighlight": {},
            "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
            "foldingRange": { "lineFoldingOnly": false },
            "selectionRange": {},
            "rangeFormatting": {},
            "onTypeFormatting": {},
            "inlayHint": { "resolveSupport": { "properties": [] } },
            "callHierarchy": {},
            "typeHierarchy": {},
            "semanticTokens": {
                "requests": { "full": true, "range": false },
                "tokenTypes": TOKEN_TYPES,
                "tokenModifiers": TOKEN_MODIFIERS,
                "formats": ["relative"],
                "overlappingTokenSupport": false,
                "multilineTokenSupport": false,
            },
        },
        "window": {
            "showMessage": { "messageActionItem": { "additionalPropertiesSupport": false } },
            "showDocument": { "support": true },
            "workDoneProgress": true,
        },
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
        // `did_open` reads it.
        if let Some(b) = self.buffers.get_mut(&buffer) {
            b.language_id = Some(target.language_id);
        }
        let mut ids = Vec::new();
        for (name, root) in target.servers {
            let Some(server_config) = config.servers.get(&name).cloned() else {
                continue;
            };
            let key = (name.clone(), root.clone(), target.sandbox.clone());
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
                            sandbox: target.sandbox.clone(),
                            pull: false,
                            pulls: HashMap::new(),
                            next_pull: 0,
                            watchers: Vec::new(),
                            root_watch: None,
                            prompts: HashMap::new(),
                            next_prompt: 0,
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
                self.schedule_pull(id, buffer);
            }
            ids.push(id);
        }
        if let Some(b) = self.buffers.get_mut(&buffer) {
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
            server.sandbox.clone(),
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

    /// Pull `buffer`'s diagnostics from a server that wants that, once
    /// changes settle.
    fn schedule_pull(&mut self, id: u64, buffer: Uuid) {
        let Some(server) = self.lsp.servers.get_mut(&id).filter(|s| s.pull) else {
            return;
        };
        server.next_pull += 1;
        let seq = server.next_pull;
        server.pulls.insert(buffer, seq);
        tokio::spawn(async move {
            tokio::time::sleep(PULL_DELAY).await;
            super::hub().lsp_pull(id, buffer, seq);
        });
    }

    fn pull_now(&mut self, id: u64, buffer: Uuid) {
        let Some(b) = self.buffers.get(&buffer) else {
            return;
        };
        let uri = config::uri(&b.path);
        let Some(server) = self.lsp.servers.get_mut(&id).filter(|s| s.ready()) else {
            return;
        };
        let sid = server.next_request;
        server.next_request += 1;
        server.pending.insert(sid, Pending::Pull(uri.clone()));
        server.send(json!({
            "jsonrpc": "2.0",
            "id": sid,
            "method": "textDocument/diagnostic",
            "params": { "textDocument": { "uri": uri } },
        }));
    }

    /// Take one source's diagnostics for `uri`; tabs get both sources'
    /// together, and tabs connecting later get them from the cache.
    fn publish(&mut self, id: u64, uri: &str, pulled: bool, items: Vec<Value>) {
        let Some(server) = self.lsp.servers.get_mut(&id) else {
            return;
        };
        let entry = server.diagnostics.entry(uri.to_owned()).or_default();
        if pulled {
            entry.pulled = items;
        } else {
            entry.pushed = items;
        }
        let params = entry.params(uri);
        if entry.pushed.is_empty() && entry.pulled.is_empty() {
            server.diagnostics.remove(uri);
        }
        let message = notification("textDocument/publishDiagnostics", params);
        let sockets: Vec<_> = server.sockets.iter().copied().collect();
        for socket in sockets {
            self.send_lsp(socket, id, message.clone());
        }
    }

    /// Tell a buffer's servers what changed.
    pub(super) fn lsp_changed(&mut self, buffer: Uuid, applied: &[Applied]) {
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
        let servers = self
            .buffers
            .get(&buffer)
            .map(|b| b.servers.clone())
            .unwrap_or_default();
        for id in servers {
            self.schedule_pull(id, buffer);
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
            let had = server.sockets.remove(&socket);
            server.waiting.retain(|(s, _)| *s != socket);
            server
                .pending
                .retain(|_, p| !matches!(p, Pending::Tab(s, _) if *s == socket));
            // Nobody is left to answer.
            if had && server.sockets.is_empty() {
                let prompts: Vec<_> = server.prompts.drain().collect();
                for (_, rid) in prompts {
                    server.send(response(rid, Value::Null));
                }
            }
        }
    }

    /// Tell tabs that prompts no longer need an answer.
    fn prompts_done(&mut self, id: u64, tokens: &[u64]) {
        let Some(server) = self.lsp.servers.get(&id) else {
            return;
        };
        let sockets: Vec<_> = server.sockets.iter().copied().collect();
        for token in tokens {
            let msg = super::to_json(&EditorServerMessage::LspPromptDone {
                server: id,
                token: *token,
            });
            for socket in &sockets {
                self.send_raw(*socket, msg.clone());
            }
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
                        let diagnostics: Vec<_> = server
                            .diagnostics
                            .iter()
                            .map(|(uri, d)| d.params(uri))
                            .collect();
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
                let found = server.pending.iter().find_map(|(sid, p)| match p {
                    Pending::Tab(s, rid) if *s == socket && rid == cancelled => Some(*sid),
                    _ => None,
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

enum Answer {
    Reply(Value),
    /// `workspace/applyEdit`, answered once files load.
    ApplyEdit,
    /// Answered later (a prompt), or not at all (the server is gone).
    Later,
}

impl super::Hub {
    /// The language servers' diagnostics for open files (under `under`, if
    /// given), for automation.
    pub fn diagnostics(&self, under: Option<&std::path::Path>) -> Vec<Value> {
        let inner = self.inner.lock().unwrap();
        let mut out = Vec::new();
        for server in inner.lsp.servers.values() {
            for (uri, diagnostics) in &server.diagnostics {
                let Some(path) = config::path(uri) else {
                    continue;
                };
                if under.is_some_and(|under| !path.starts_with(under)) {
                    continue;
                }
                for d in diagnostics.pushed.iter().chain(&diagnostics.pulled) {
                    let severity = match d["severity"].as_u64() {
                        Some(1) => "error",
                        Some(2) => "warning",
                        Some(3) => "info",
                        _ => "hint",
                    };
                    out.push(json!({
                        "path": crate::fs_ops::client_path(&path),
                        // 1-based, as editors show them.
                        "line": d["range"]["start"]["line"].as_u64().map(|l| l + 1),
                        "column": d["range"]["start"]["character"].as_u64().map(|c| c + 1),
                        "severity": severity,
                        "message": d["message"],
                        "source": d.get("source").cloned().unwrap_or_else(|| server.name.clone().into()),
                    }));
                }
            }
        }
        out
    }

    fn lsp_pull(&self, id: u64, buffer: Uuid, seq: u64) {
        let mut inner = self.inner.lock().unwrap();
        let current = inner
            .lsp
            .servers
            .get_mut(&id)
            .and_then(|s| s.pulls.get(&buffer).copied());
        if current != Some(seq) {
            return;
        }
        if let Some(server) = inner.lsp.servers.get_mut(&id) {
            server.pulls.remove(&buffer);
        }
        inner.pull_now(id, buffer);
    }

    /// A tab answered `window/showMessageRequest`; the first answer wins.
    pub(super) fn lsp_prompt_answer(&self, id: u64, token: u64, action: Option<String>) {
        let mut inner = self.inner.lock().unwrap();
        let Some(server) = inner.lsp.servers.get_mut(&id) else {
            return;
        };
        let Some(rid) = server.prompts.remove(&token) else {
            return;
        };
        let result = action.map_or(Value::Null, |title| json!({ "title": title }));
        server.send(response(rid, result));
        inner.prompts_done(id, &[token]);
    }

    fn lsp_files_changed(&self, id: u64, generation: u32, changes: Vec<(PathBuf, u8)>) {
        let inner = self.inner.lock().unwrap();
        let Some(server) = inner.lsp.servers.get(&id) else {
            return;
        };
        if server.generation != generation || !server.ready() {
            return;
        }
        let watchers: Vec<_> = server.watchers.iter().flat_map(|(_, w)| w).collect();
        if let Some(params) = watch::changes(&watchers, &changes) {
            server.send(notification("workspace/didChangeWatchedFiles", params));
        }
    }

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
                match server.pending.remove(&sid) {
                    Some(Pending::Tab(socket, tab_id)) => {
                        let mut reply = message;
                        reply["id"] = tab_id;
                        inner.send_lsp(socket, id, reply);
                    }
                    Some(Pending::Pull(uri)) => {
                        let result = &message["result"];
                        if result["kind"] == "full" {
                            let items = result["items"].clone();
                            let items = items.as_array().cloned().unwrap_or_default();
                            inner.publish(id, &uri, true, items);
                        }
                    }
                    None => {}
                }
            }
            (Some(method), Some(rid)) => {
                let answer = inner.answer_server(id, &method, rid.clone(), &message["params"]);
                match answer {
                    Answer::Reply(reply) => {
                        if let Some(server) = inner.lsp.servers.get(&id) {
                            server.send(reply);
                        }
                    }
                    Answer::Later => {}
                    // Loads files first.
                    Answer::ApplyEdit => {
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
        // Stopped after a failed `initialize`: not a crash.
        if server.generation != generation || server.handle.is_none() {
            return;
        }
        tracing::info!(server = server.name, "{}", exit.message);
        server.handle = None;
        server.progress.clear();
        // The process's registrations and prompts went with it; a restarted
        // one registers again, under its own generation.
        server.watchers.clear();
        server.root_watch = None;
        let prompts: Vec<u64> = server.prompts.drain().map(|(token, _)| token).collect();
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
        for (_, p) in pending {
            if let Pending::Tab(socket, rid) = p {
                inner.send_lsp(socket, id, error(rid, REQUEST_FAILED, &exit.message));
            }
        }
        for (socket, rid) in waiting {
            inner.send_lsp(socket, id, error(rid, REQUEST_FAILED, &exit.message));
        }
        inner.prompts_done(id, &prompts);
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
        inner.lsp.by_key.remove(&(
            server.name.clone(),
            server.root.clone(),
            server.sandbox.clone(),
        ));
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
        server.pull = !result["capabilities"]["diagnosticProvider"].is_null();
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
            self.schedule_pull(id, buffer);
        }
        for (socket, rid) in waiting {
            self.send_lsp(socket, id, response(rid, result.clone()));
        }
        self.send_lsp_statuses();
    }

    /// Answer a request the server makes of its client.
    fn answer_server(&mut self, id: u64, method: &str, rid: Value, params: &Value) -> Answer {
        let Some(server) = self.lsp.servers.get_mut(&id) else {
            return Answer::Later;
        };
        Answer::Reply(match method {
            "workspace/applyEdit" => return Answer::ApplyEdit,
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
            "client/registerCapability" => {
                for registration in params["registrations"].as_array().into_iter().flatten() {
                    if registration["method"] != "workspace/didChangeWatchedFiles" {
                        continue;
                    }
                    let watchers = registration["registerOptions"]["watchers"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(watch::FileWatcher::parse)
                        .collect();
                    let rid = registration["id"].as_str().unwrap_or_default().to_owned();
                    server.watchers.push((rid, watchers));
                }
                if !server.watchers.is_empty() && server.root_watch.is_none() {
                    let generation = server.generation;
                    server.root_watch = watch::RootWatch::start(&server.root, move |changes| {
                        super::hub().lsp_files_changed(id, generation, changes)
                    });
                }
                response(rid, Value::Null)
            }
            "client/unregisterCapability" => {
                // The spec's spelling.
                for gone in params["unregisterations"].as_array().into_iter().flatten() {
                    let gone = gone["id"].as_str().unwrap_or_default();
                    server.watchers.retain(|(rid, _)| rid != gone);
                }
                if server.watchers.is_empty() {
                    server.root_watch = None;
                }
                response(rid, Value::Null)
            }
            "window/showMessageRequest" => {
                if server.sockets.is_empty() {
                    response(rid, Value::Null)
                } else {
                    let token = server.next_prompt;
                    server.next_prompt += 1;
                    server.prompts.insert(token, rid);
                    let msg = super::to_json(&EditorServerMessage::LspPrompt {
                        server: id,
                        token,
                        level: params["type"].as_u64().unwrap_or(3) as u8,
                        message: params["message"].as_str().unwrap_or_default().to_owned(),
                        actions: params["actions"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|a| a["title"].as_str().map(str::to_owned))
                            .collect(),
                    });
                    let sockets: Vec<_> = server.sockets.iter().copied().collect();
                    for socket in sockets {
                        self.send_raw(socket, msg.clone());
                    }
                    return Answer::Later;
                }
            }
            "window/showDocument" => {
                let shown = !server.sockets.is_empty();
                let msg = super::to_json(&EditorServerMessage::LspShowDocument {
                    uri: params["uri"].as_str().unwrap_or_default().to_owned(),
                    external: params["external"].as_bool().unwrap_or(false),
                    selection: params.get("selection").cloned(),
                });
                let sockets: Vec<_> = server.sockets.iter().copied().collect();
                for socket in sockets {
                    self.send_raw(socket, msg.clone());
                }
                response(rid, json!({ "success": shown }))
            }
            "workspace/diagnostic/refresh" => {
                let buffers: Vec<_> = server.buffers.iter().copied().collect();
                for buffer in buffers {
                    self.schedule_pull(id, buffer);
                }
                response(rid, Value::Null)
            }
            "workspace/semanticTokens/refresh"
            | "workspace/inlayHint/refresh"
            | "workspace/codeLens/refresh" => {
                // Tabs ask again.
                let refresh = notification("$/btmux/refresh", json!({ "method": method }));
                let sockets: Vec<_> = server.sockets.iter().copied().collect();
                for socket in sockets {
                    self.send_lsp(socket, id, refresh.clone());
                }
                response(rid, Value::Null)
            }
            "window/workDoneProgress/create" => response(rid, Value::Null),
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
                    let items = params["diagnostics"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default();
                    self.publish(id, uri, false, items);
                }
                return;
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

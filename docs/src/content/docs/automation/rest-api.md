---
title: "REST API"
sidebar:
  order: 3
---

Manage sessions, windows, and panes over HTTP under `/api`. Send the
[access token](index.md#authentication) with every request.

```sh
api="${BTMUX_API_URL:-http://127.0.0.1:8004}/api"
auth="Authorization: Bearer $BTMUX_AUTH_TOKEN"

# Create a session; the reply has its first pane's ID
curl -H "$auth" -X POST "$api/sessions" --json '{"name":"build"}'

# Type into a pane, then read its screen
curl -H "$auth" -X POST "$api/panes/<pane-id>/input" --json '{"text":"echo hi","keys":["Enter"]}'
curl -H "$auth" "$api/panes/<pane-id>/capture?lines=20"

# Bring a pane to the front in every client
curl -H "$auth" -X POST "$api/panes/<pane-id>/focus"
```

## Input and output

- **`input`** sends `text` verbatim, then `keys`: tmux-style key names such as
  `Enter`, `Tab`, `Escape`, `Space`, `BSpace`, `Up`, `PageDown`, `F5`, `C-c`,
  or `M-x`. Cursor keys follow the program's cursor key mode, as in tmux.
- **`capture`** returns the pane as rendered plain text with wrapped lines
  joined, like `tmux capture-pane -pJ`. It covers the visible screen;
  `scrollback=true` adds all retained history above it, and `history=N` at
  most N rows of it. `lines=N` keeps the last N lines.
- **`output`** returns the raw bytes instead, escapes included.

## Panes and windows

- **`split`** takes `direction` (`left`, `right`, `up`, or `down`: where the
  new pane goes), optional `cwd`, and `select` (default true), and returns the
  new `pane_id`. Creating a session or window returns its `pane_id` too.
- **`focus`** switches every connected client to the pane's window and
  session. It replies 204, or 404 for an unknown pane.
- **`open-file-browser`** does the same and then opens a dialog there. Its
  `mode` is `"files"` (default), `"git"`, or `"process"`, as `<prefix> + f`,
  `g`, and `t` open. In file mode, `path` is the directory and optional
  `focus_file` names an entry in it to focus.

## Endpoints

| Method and path                                                 | Purpose                                                       |
| --------------------------------------------------------------- | ------------------------------------------------------------- |
| `GET/POST/DELETE /api/sessions`                                 | List, create, or clear sessions                               |
| `GET/DELETE /api/sessions/<session-id>`                         | Inspect or kill a session                                     |
| `POST /api/sessions/<session-id>/rename`                        | Rename a session                                              |
| `POST /api/sessions/<session-id>/windows`                       | Create a window                                               |
| `POST /api/sessions/<session-id>/windows/{switch,rename,close}` | Operate on a session's active window                          |
| `DELETE /api/windows/<window-id>`                               | Kill a window by ID                                           |
| `POST/DELETE /api/sessions/<session-id>/panes/...`              | Split, kill, select, navigate, cycle, swap, or zoom panes     |
| `POST /api/sessions/<session-id>/layout`                        | Select a named pane layout                                    |
| `POST /api/sessions/<session-id>/layout/next`                   | Cycle to the next pane layout                                 |
| `GET /api/panes`                                                | List every pane with its window and session                   |
| `POST /api/panes/<pane-id>/split`                               | Split a pane in any window                                    |
| `POST/DELETE /api/panes/<pane-id>/worktree`                     | Create a Git worktree with a window, or remove the pane's     |
| `POST /api/panes/<pane-id>/select`                              | Make a pane and its window active                             |
| `DELETE /api/panes/<pane-id>`                                   | Kill a pane, closing its window or session if it was the last |
| `POST /api/windows/<window-id>/{select,rename}`                 | Make a window active, or rename it                            |
| `POST /api/panes/<pane-id>/input`                               | Send text and keys to a pane, spawning its shell if necessary |
| `GET /api/panes/<pane-id>/capture`                              | Read the rendered screen or scrollback as text                |
| `GET /api/panes/<pane-id>/output`                               | Read scrollback bytes, including ANSI escapes                 |
| `GET /api/agents`                                               | List panes running an agent, with their states                |
| `GET /api/panes/<pane-id>/wait`                                 | Wait for the pane's agent to reach a state                    |
| `POST /api/panes/<pane-id>/prompt`                              | Submit a prompt to the pane's agent and wait for its turn     |
| `POST /api/panes/<pane-id>/popup`                               | Run a program in a [popup](popups.md) over the pane           |
| `POST /api/notify`                                              | Show a notification, not tied to a pane, in every client      |
| `POST/DELETE /api/panes/<pane-id>/notify`                       | Report agent events or set/clear a notification               |
| `POST /api/panes/<pane-id>/open-file-browser`                   | Open the file browser at a path, in the pane's session        |
| `POST /api/panes/<pane-id>/focus`                               | Switch every client to the pane's window and session          |
| `GET/POST/DELETE /api/panes/<pane-id>/agent-status`             | Explain, set, or clear semantic agent status                  |
| `GET /api/system`                                               | Describe the server host (OS, CPU, memory, disks, shell, ...) |
| `GET /api/health`                                               | Report which optional programs the server found and whether they run |
| `POST /api/server/shutdown`                                     | Stop the server, closing every shell                          |

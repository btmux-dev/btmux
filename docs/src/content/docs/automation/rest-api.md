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

The [API reference](/automation/api/) gives each endpoint's parameters,
request and response bodies, and status codes. It is generated from the
server's code as an OpenAPI 3.1 document, which clients and tools can read
too: a server serves the one for its version at `/api/openapi.json`, and this
site [the latest](/openapi.json). Endpoints the btmux app uses for itself,
such as file previews and editors, aren't listed.

Every endpoint can reply 401 for a missing or wrong token. Error bodies, if
any, are plain text. Malformed IDs and JSON bodies get 400, a body without
`Content-Type: application/json` 415, and one that doesn't match its schema
422.

<!-- api-endpoints -->

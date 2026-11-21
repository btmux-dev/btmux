---
title: "Automation"
sidebar:
  order: 1
  label: "Overview"
---

The CLI, REST API, and MCP server use the same btmux server as the UI, at
`127.0.0.1:8004` by default.

| To                                         | Use                                   |
| ------------------------------------------ | ------------------------------------- |
| Script panes and windows from a shell      | [tmux-style commands](cli.md)         |
| Call btmux over HTTP                       | [REST API](rest-api.md)               |
| Give an agent tools for btmux              | [MCP server](mcp.md)                  |
| Ask the user something from a script       | [Widgets](widgets.md)                 |
| Run a program over a pane                  | [Popups](popups.md)                   |
| Tell the user something                    | [Notifications](notifications.md)     |
| Hand work to agents and wait for it        | [Orchestrating agents](../agents/orchestrating.md) |
| React to changes as they happen            | [Events](events.md)                   |

## Authentication

The `btmux` commands find the server and its token themselves. HTTP and MCP
requests need the access token in a header:

```text
Authorization: Bearer $BTMUX_AUTH_TOKEN
```

Inside a pane, `BTMUX_AUTH_TOKEN` and `BTMUX_API_URL` are already set. Outside
one, read the token from its file:

```sh
export BTMUX_AUTH_TOKEN="$(cat "${XDG_STATE_HOME:-$HOME/.local/state}/btmux/state.token")"
```

A [profile](../configuration/profiles.md) has its own
[token file](../configuration/access.md#token-files).

:::caution
The token gives full control of your shells and access to your files.
:::

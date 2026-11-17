---
title: "Automation"
sidebar:
  order: 1
  label: "Overview"
---

Scripts and agents can drive btmux the same ways you do. Everything here is
served by the btmux server itself, on the port the UI uses (`127.0.0.1:8004`
by default).

| To                                         | Use                                   |
| ------------------------------------------ | ------------------------------------- |
| Script panes and windows from a shell      | [tmux-style commands](cli.md)         |
| Call btmux over HTTP                       | [REST API](rest-api.md)               |
| Give an agent tools for btmux              | [MCP server](mcp.md)                  |
| Ask the user something from a script       | [Widgets](widgets.md)                 |
| Run a program over a pane                  | [Popups](popups.md)                   |
| Tell the user something                    | [Notifications](notifications.md)     |
| Hand work to agents and wait for it        | [Orchestrating agents](../agents/orchestrating.md) |

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

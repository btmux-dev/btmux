---
title: "Automation"
sidebar:
  order: 1
  label: "Overview"
---

The REST API, MCP server, and web UI share the same host and port (by default,
`127.0.0.1:8004`). No separate process is required.

## Authentication

All HTTP and MCP calls require the instance's access token. Inside a btmux pane,
`BTMUX_AUTH_TOKEN` is already set. Outside a pane, load the default instance's
token before running the examples below:

```sh
export BTMUX_AUTH_TOKEN="$(cat "${XDG_STATE_HOME:-$HOME/.local/state}/btmux/state.token")"
```

For a named profile or development instance, use its
[profile-specific token file](../configuration/access.md#token-files). If the server was
started with `BTMUX_AUTH_TOKEN` set, use that value instead.

Send `Authorization: Bearer $BTMUX_AUTH_TOKEN` with every request. Existing MCP
client configurations and previously installed hook snippets also need this
header.

:::caution
The token grants control of shells and access to files available to the btmux
user.
:::

---
title: "Access tokens"
sidebar:
  order: 6
---

Every request to a btmux server needs its access token, even on localhost.
Browsers ask for it once as a password; the desktop app and the `btmux`
commands handle it for you.

## Token files

Unless
`BTMUX_AUTH_TOKEN` is set, btmux creates an owner-only `state.token` beside the
profile's `state.json` and prints its path on startup.

| Server                 | Default token file                      |
| ---------------------- | --------------------------------------- |
| `btmux`                | `~/.local/state/btmux/state.token`      |
| `btmux --profile NAME` | `~/.local/state/btmux/NAME/state.token` |

On Windows, `%LOCALAPPDATA%` replaces `~/.local/state`. If `XDG_STATE_HOME` is
set, it replaces either. A background service uses the same files, so you can
read its token without reading service logs.

## Rotating the token

To rotate a generated token, stop the server, delete its token file, and start
it again. Alternatively, start the server with `BTMUX_AUTH_TOKEN` set to at least
4 ASCII letters, digits, `-`, or `_`; this overrides the token file.

:::caution
The token gives full control of your shells and access to your files. Never
put it in a URL.
:::

The browser session uses an HttpOnly, SameSite=Strict cookie; HTTPS public URLs
use Secure cookies. See [Automation authentication](../automation/index.md#authentication)
for API, MCP, and agent-hook clients.

## Reverse proxies

To serve btmux at a public address, put it behind a reverse proxy that
terminates HTTPS, and tell btmux that address:

```sh
btmux --public-url https://terminal.example.com
```

The proxy must pass the public `Host` header through and forward WebSocket
upgrades. btmux trusts only the addresses you name: `X-Forwarded-*` headers
grant nothing. Don't serve it over plain HTTP, which encrypts neither the
token nor your terminals.

To reach btmux from your other devices without a proxy, see
[Remote access](../features/remote-access.mdx).

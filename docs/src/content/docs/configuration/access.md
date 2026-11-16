---
title: "Access and remote access"
sidebar:
  order: 3
---

## Token files

Every request needs the access token, even on localhost. Unless
`BTMUX_AUTH_TOKEN` is set, btmux creates an owner-only `state.token` beside the
profile's `state.json` and prints its path on startup.

| Instance               | Default token file                      |
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
The token grants access to the instance's shells, sessions, and files available
to the btmux user. Never put it in URLs.
:::

The browser session uses an HttpOnly, SameSite=Strict cookie; HTTPS public URLs
use Secure cookies. See [Automation authentication](../automation/index.md#authentication)
for API, MCP, and agent-hook clients.

## Reverse proxies

For a remote deployment, terminate HTTPS at a trusted reverse proxy and pass its
external origin explicitly, for example `--public-url https://terminal.example.com`.
Preserve the public Host header (or use the configured backend authority) and
forward WebSocket upgrades. Forwarded headers never implicitly grant trust.
Plain HTTP does not encrypt the access token or terminal traffic.

To reach btmux from your other devices without a proxy, see
[Phones, tablets, and Tailscale](../features/phones-and-tailscale.mdx).

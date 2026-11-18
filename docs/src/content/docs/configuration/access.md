---
title: "Access tokens"
sidebar:
  order: 6
---

btmux requires an access token even on localhost. Enter it as the browser
password. The desktop app and CLI handle it automatically.

## Token files

Unless `BTMUX_AUTH_TOKEN` is set, btmux creates an owner-only `state.token`
beside the profile's `state.json` and prints its path at startup.

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

Terminate HTTPS at the reverse proxy and give btmux the public address.

```sh
btmux --public-url https://terminal.example.com
```

Preserve the public `Host` header and forward WebSocket upgrades. btmux
trusts the configured addresses and ignores `X-Forwarded-*` headers for
that check. Use HTTPS to encrypt the token and terminal traffic.

To reach btmux from your other devices without a proxy, see
[Remote access](../features/remote-access.mdx).

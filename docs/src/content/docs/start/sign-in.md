---
title: "Signing in and remote access"
sidebar:
  order: 2
---

## First sign-in

Start `btmux`, then open `http://127.0.0.1:8004`. Access requires a token even
on localhost. Unless `BTMUX_AUTH_TOKEN` is set, btmux creates an owner-only
`state.token` beside the profile's `state.json` and prints its path on startup.

| Instance                        | Default token file                      |
| ------------------------------- | --------------------------------------- |
| `btmux`                         | `~/.local/state/btmux/state.token`      |
| `btmux --profile NAME`          | `~/.local/state/btmux/NAME/state.token` |
| `just dev` / `just dev-backend` | `~/.local/state/btmux/dev/state.token`  |

These paths apply on both macOS and Linux. On Windows, `%LOCALAPPDATA%`
replaces `~/.local/state` (`%LOCALAPPDATA%\btmux\state.token`). If
`XDG_STATE_HOME` is set, it replaces either. For the default instance, read the
token with:

```sh
cat "${XDG_STATE_HOME:-$HOME/.local/state}/btmux/state.token"
```

or, in PowerShell on Windows:

```powershell
Get-Content "$env:LOCALAPPDATA\btmux\state.token"
```

Enter `btmux` as the browser username and the file's contents as the password.
The development frontend at `http://localhost:5173` instead shows an access-token
form; paste the `dev` profile's token there. A background service uses the same
file locations, so you can retrieve its token without reading service logs.

## Token management

Tokens survive restarts. To rotate a generated token, stop the server, delete
its token file, and restart it to generate a new one. Alternatively, start the
server with `BTMUX_AUTH_TOKEN` set to at least 4 ASCII letters, digits, `-`, or
`_`; this overrides the token file.

The token grants access to the instance's shells, sessions, and files available
to the btmux user. Never put credentials in URLs. The browser session uses an
HttpOnly, SameSite=Strict cookie; HTTPS public URLs use Secure cookies.
See [Automation authentication](../automation/index.md#authentication) for API, MCP,
and agent-hook clients.

Run concurrent instances with different `--profile` names. A file lock prevents
two instances from overwriting the same saved sessions. `just dev` and
`just dev-backend` use the `dev` profile automatically.

## Remote access

For a remote deployment, terminate HTTPS at a trusted reverse proxy and pass its
external origin explicitly, for example `--public-url https://terminal.example.com`.
Preserve the public Host header (or use the configured backend authority) and
forward WebSocket upgrades. Forwarded headers never implicitly grant trust.
Plain HTTP does not encrypt the access token or terminal traffic.

To reach btmux from your other devices without a proxy, see
[Phones, tablets, and Tailscale](../features/phones-and-tailscale.md).

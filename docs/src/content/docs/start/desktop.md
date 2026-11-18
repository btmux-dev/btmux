---
title: "Desktop app"
sidebar:
  order: 4
---

The desktop app is a window onto a btmux server. It can start its own, or
connect to any server running on this machine or another one.

## Choosing a server

On startup the app looks for btmux servers running on this machine. If it
finds none, it starts its own and connects. Otherwise it shows a picker:

- **The desktop server**, the app's own, with **Start**, **Connect**, or
  **Stop**.
- **Servers started with the `btmux` command**, listed by their command line
  (`btmux`, `btmux --profile work`). Connecting shows the same sessions,
  shells, and scrollback a browser would.
- **Remote servers** you have added.

Return to the picker with **File → Switch Server…** (⇧⌘O) on macOS, or
`desktop: switch server` in the command palette. The app also returns to it
when the server stops responding.

Closing the app never stops a server it only connected to.

## Keeping shells running

By default the desktop server stops when the app quits. Its sessions and
layout are saved, but running programs end.

When starting it, choose **Keep it running after btmux quits** to keep shells
alive until you stop the server or log out. The choice is remembered. Such a
server stays on the app version that started it. After an app update, its card
shows **Upgrade**, which moves it to the new version in place, keeping its
shells (macOS and Linux). The server refuses if the two versions hand over
their state differently, and says so; then, as on Windows, stop and start it,
which closes its shells. It doesn't start at login. For that, install the
[service](installation.mdx#run-it-as-a-service) and connect to it instead.

**Stop** asks first, because it closes the server's shells.

## Other machines

**Add server** in the picker takes an address and
[access token](../configuration/access.md), for example of a server started
with `btmux --tailscale` on another machine. An address without a scheme means
HTTPS. The server must accept that address: start it with `--tailscale` or
`--public-url <address>`.

To share the desktop server itself, check **Share it on my tailnet** before
starting it (only for a server kept running). It keeps its port, so its
tailnet URL stays the same. See [Remote access](../features/remote-access.mdx).

Tokens are stored in an owner-only file in the app's settings folder and never
appear in URLs.

## Windows and zoom

**File → New Window** (⌘N) on macOS, or `desktop: new window` in the command
palette, opens another window on the picker, so windows can show the same
server or different ones. On Linux and Windows, launching the app again does
the same.

Pages zoom as in a browser: ⌘=, ⌘-, and ⌘0 on macOS, Ctrl elsewhere. The
level applies to every window and is kept across restarts.

## Notifications

While the window is hidden, attention and error
[notifications](../agents/index.md#notifications) from connected servers appear
as OS notifications. Clicking one focuses the app and its pane.

## The btmux command

The app's server is the `btmux` command, which it also installs:

- **macOS:** choose **btmux → Install Command-Line Tool…**. It links
  `/usr/local/bin/btmux` into the app, so app updates update it, and asks for
  an administrator password. Keep the app in a lasting place such as
  `/Applications`. Choose it again to remove the link.
- **Linux:** the `.deb` installs `/usr/bin/btmux`.
- **Windows:** the installer adds `bin\btmux.exe` in its folder to your user
  `PATH`, and removes it on uninstall. Open a new terminal to use it. Stop
  `btmux` servers running from it before installing an update or uninstalling.

A `btmux` earlier in `PATH` still wins. After updating the app, `btmux upgrade`
moves a running command-line server to the new version.

## Versions

The app and a server it connects to can be different versions. When they
differ, the app warns on connect, and **About btmux** lists both.

## Transparency

On macOS the window is transparent with a theme-colored tint. Set
`desktop-background-opacity` in `config.toml` from clear (`0`) to opaque (`1`);
the default is `0.95`. Linux and Windows windows are opaque.

Building and releasing the app is covered in its
[README](../../../../../desktop/README.md).

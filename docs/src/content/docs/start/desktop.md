---
title: "Desktop app"
sidebar:
  order: 4
---

The desktop app can start its own server or connect to a local or remote server.

## Choosing a server

If there are no running local servers or saved remote servers, the app starts
its own server and connects. Otherwise, the picker lists local servers by
command line and shows your saved remote servers. Use **Start**, **Connect**,
or **Stop** to manage the desktop server.

Return to the picker with **File → Switch Server…** (Cmd+Shift+O) on macOS, or
`desktop: switch server` in the command palette. The app also returns to it
when the server stops responding.

Closing the app never stops a server it only connected to.

## Keeping shells running

By default the desktop server stops when the app quits. Its sessions and
layout are saved, but running programs end.

Choose **Keep it running after btmux quits** at server startup to keep shells
alive until you stop the server or log out. The app remembers your choice.
The server keeps the app version it started with. When the app is newer, the
server's card shows **Upgrade**, which moves it to the app's version in place
and keeps its shells (macOS and Linux). If the two versions can't hand over in
place, the server refuses and says why. Then stop and start it, which closes
its shells, as on Windows. For automatic startup at login, connect to a
[service](installation.mdx#run-it-as-a-service).

**Stop** asks first, because it closes the server's shells.

## Other machines

In the picker, choose **Add server** and enter its address and
[access token](../configuration/access.md). The app assumes HTTPS if you omit
the scheme. Start the remote server with `--tailscale` or `--public-url <address>`
to allow that address.

To share the desktop server itself, check **Share it on my tailnet** before
starting it (only for a server kept running). It keeps its port, so its
tailnet URL stays the same. See [Remote access](../features/remote-access.mdx).

Tokens are stored in an owner-only file in the app's settings folder and never
appear in URLs.

## Windows and zoom

**File → New Window** (Cmd+N) on macOS, or `desktop: new window` in the command
palette, opens the picker in a new window. On Linux and Windows, launch the
app again to open another window.

Zoom with Cmd+=, Cmd+-, and Cmd+0 on macOS, or Ctrl elsewhere. All windows
share the saved zoom level.

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

The app warns on connect if its version differs from the server's.
**About btmux** lists both versions.

## Transparency

On macOS the window is transparent with a theme-colored tint. Set
`desktop-background-opacity` in `config.toml` from clear (`0`) to opaque (`1`);
the default is `0.95`. Linux and Windows windows are opaque.

Building and releasing the app is covered in its
[README](../../../../../desktop/README.md).

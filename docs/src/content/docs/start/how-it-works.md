---
title: "How btmux works"
sidebar:
  order: 3
---

## A server and its clients

The **server** runs your shells. Each desktop window or browser tab is a
**client**. Reconnect from any client while the server is running to resume
your session.

The desktop app can start its own server. You can also
[connect to a server on another machine](../features/remote-access.mdx).

## Sessions, windows, panes

A **session** groups **windows**, which work like tabs. Each window contains
one or more **panes**, each with its own shell.

## What clients share

- Clients viewing the same session share its active window, active pane, and
  terminal output. The color scheme is shared across all clients.
- Each client chooses its own session.
- Pane size follows the client that last typed, pasted, or clicked in the pane.

## What survives

| You do this                              | Layout | Running programs | Scrollback |
| ---------------------------------------- | ------ | ---------------- | ---------- |
| Close a browser tab                      | Kept   | Kept             | Kept       |
| `btmux upgrade` to a new version         | Kept   | Kept             | Kept       |
| Restart the server, or reboot            | Kept   | Lost             | Lost       |
| Close the last desktop window (its own server) | Kept   | Lost, unless you [keep it running](desktop.md#keeping-shells-running) | Lost, likewise |

After a restart, each pane starts a fresh shell in the directory it was in.
See [how in-place upgrades work](../internals/upgrade.md).

## Over a pane

The [editors](../features/editors.md) and the
[browser](../features/browser.md) cover the active pane. The
[file browser](../features/files-and-git.md), process viewer, and agent view
open as dialogs.

## Profiles and access

Browsers sign in with the server's [access token](../configuration/access.md).
The desktop app handles sign-in for you. Use [profiles](../configuration/profiles.md)
to run servers with separate sessions.

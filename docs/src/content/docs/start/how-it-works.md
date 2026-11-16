---
title: "How btmux works"
sidebar:
  order: 3
---

A few ideas explain most of what btmux does.

## A server and its clients

Your shells run in a **server**: one background process on one machine. What
you look at is a **client**: a desktop app window, a browser tab, or a phone.
Clients only display what the server has. Close one and nothing stops; open
another and it shows the same thing.

The desktop app starts a server for you, so you may never notice it. On a
remote machine you run the server yourself and
[connect to it](../features/remote-access.mdx).

## Sessions, windows, panes

The server holds **sessions**. A session has **windows**, like tabs, and a
window is split into **panes**, each running a shell. One session per project
is a common setup.

## What clients share

Several clients can show the same session at once.

- **Shared:** which window and pane are active, the color scheme, and
  everything on screen. Type in one client and the others see it.
- **Per client:** which session is shown. Two clients can look at different
  sessions.
- **Pane size** follows the client that last typed or clicked in the pane.
  Others show that size.

## What survives

| You do this                              | Layout | Running programs | Scrollback |
| ---------------------------------------- | ------ | ---------------- | ---------- |
| Close the window or browser tab          | Kept   | Kept             | Kept       |
| `btmux upgrade` to a new version         | Kept   | Kept             | Kept       |
| Restart the server, or reboot            | Kept   | Lost             | Lost       |
| Quit the desktop app (its own server)    | Kept   | Lost, unless you [keep it running](desktop.md#keeping-shells-running) | Lost, likewise |

After a restart, each pane starts a fresh shell in the directory it was in.
[How in-place upgrades work](../internals/upgrade.md) explains the second row.

## Over a pane

The [editors](../features/editors.md) and the
[browser](../features/browser.md) open *over* the active pane: they cover that
one pane and leave the rest of the window as it is. The
[file browser](../features/files-and-git.md), the process viewer, and the
agent view are dialogs over the whole window.

## Profiles and access

Every server has an [access token](../configuration/access.md); browsers sign
in with it, and the desktop app handles it for you. To run separate servers
with separate sessions, use [profiles](../configuration/profiles.md).

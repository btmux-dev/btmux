---
title: "VS Code"
sidebar:
  order: 7
---

With `editor = "vscode"`, `<prefix> + e` opens VS Code
[over the active pane](editors.md#over-a-pane), in the pane's Git worktree
(its root, not the subfolder the pane is in), else the pane's folder. On touch
screens, the key bar has a button to hide it.

Requires macOS or Linux, and either VS Code or `openvscode-server` on the server
(see [Server](#server)). Webviews such as the Markdown preview need internet
access.

btmux still takes the prefix key while VS Code has focus. To give VS Code that
key itself, press it twice: with the default prefix, `Ctrl+A Ctrl+A` sends one
`Ctrl+A` to VS Code.

VS Code reloads only when shown over a pane in another folder, which ends its
terminals and tasks. Over a pane connected to another host (ssh), it keeps its
folder.

Over a pane of a [sandboxed session](sandbox.md), VS Code still runs on the
host: its terminals, tasks, and extensions act on files the sandbox can change,
with your account. btmux warns about this once per session.

Files open in the VS Code windows showing that pane's folder, in every client
showing it, and not in windows of other folders. With another `editor`, VS Code
is still [one click or command away](editors.md#using-both).

## Server

```toml
[vscode]
server = "code"
```

`server` picks the server, read when it starts:

- `"code"` (default) runs `code serve-web` from an installed VS Code, found on
  the server's `PATH`, your shell's, or in `/Applications` on macOS. It downloads the matching
  server on first use. The first time, btmux asks you to accept the VS Code
  Server license (once per profile).
- `"openvscode-server"` uses `openvscode-server` from `PATH`.

When btmux's `PATH` lacks the server, as a service's may, VS Code runs with your
shell's, so extensions find the same tools as your panes.

User data and extensions are kept per profile, in a `vscode` directory beside
the [profile's state file](../configuration/profiles.md).

## Sharing desktop settings and extensions

```toml
[vscode]
desktop = true
# profile = "Work"
# settings = "~/.config/Code/User/settings.json"
# keybindings = "~/.config/Code/User/keybindings.json"
# extensions = ["~/.vscode/extensions"]
```

`desktop = true` shares desktop VS Code's settings, key bindings, and
extensions with btmux's VS Code. It reads them from the default locations:

| What                     | macOS                                     | Linux                        |
| ------------------------ | ----------------------------------------- | ---------------------------- |
| Settings and key bindings | `~/Library/Application Support/Code/User` | `$XDG_CONFIG_HOME/Code/User` |
| Extensions               | `~/.vscode/extensions`                    | `~/.vscode/extensions`       |

- `settings`, `keybindings`, and `extensions` override those paths, and work
  without `desktop`: for VSCodium or Insiders, say. `extensions` lists
  extension folders, or folders of them.
- `profile` shares a desktop profile by name instead of the default one, with
  whatever it takes from the default profile. A profile with no extensions of
  its own yet shares none.
- With Home Manager, `programs.vscode` writes the default locations, so
  `programs.btmux.settings.vscode.desktop = true` is all it takes.

### How settings stay in sync

btmux's VS Code keeps its own settings, in the browser. Each time a VS Code
window loads, btmux reads the shared files and applies what changed in them
since the last load.

- A change you make in btmux's VS Code stays until the shared file changes the
  same setting or binding.
- Key bindings follow the shared file's order, which decides which of two
  bindings for a key wins. Bindings added in btmux's VS Code keep their place
  among them.

### How extensions stay in sync

Shared extensions are linked, not copied, when VS Code starts and again
whenever desktop VS Code installs, updates, or removes one.

- Desktop VS Code updates them; btmux's doesn't.
- Uninstalling one in btmux's VS Code lasts until it starts again.
- Extensions you install in btmux's VS Code stay, unless desktop VS Code has
  the same one.
- With `openvscode-server`, Microsoft's and GitHub's closed-source extensions
  (Pylance, C/C++, C# Dev Kit, Copilot) aren't shared, as they run only in
  Microsoft's VS Code.

## Extensions that don't work

VS Code in the browser runs extensions on the server, in Node.js, or in the
browser, so some desktop extensions can't run:

- Extensions that run only in the desktop window (`extensionKind: ["ui"]`)
  without a [web version](https://code.visualstudio.com/api/extension-guides/web-extensions).
- Native modules built for desktop VS Code's Electron, and extensions that use
  Electron itself; see [remote extensions](https://code.visualstudio.com/api/advanced-topics/remote-extensions#using-native-nodejs-modules).
- Extensions that serve a page on `localhost` (previews, sign-in) and open it
  in the browser: VS Code's port forwarding doesn't work here, so from another
  device `localhost` is that device. On the server's own machine they work.
- Keys typed in a webview (the Markdown preview, extension panels) reach it
  before btmux, so after the prefix, the next key reaches it too.

Signing in to GitHub or Microsoft accounts returns from their sites to btmux,
which loads the page again from btmux's own site so the browser sends its
sign-in cookie.

## Color scheme and font

A bundled extension syncs btmux's color scheme and terminal font into VS Code,
over shared settings too; the editor uses the font at 80% of its size. Turn off
`btmux.syncTheme` or `btmux.syncFont` in VS Code's settings to keep your own;
turned off in shared settings, VS Code gets their color scheme or font back.

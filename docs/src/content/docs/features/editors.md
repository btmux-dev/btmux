---
title: "Editors"
sidebar:
  order: 4
---

`<prefix> + e` opens an editor over the active pane, and the
[file browser and Git mode](files-and-git.md) open files there. `editor` in
[`config.toml`](../configuration/index.md) chooses which:

| `editor`   | Opens                                                             |
| ---------- | ----------------------------------------------------------------- |
| `"native"` | btmux's own [editor](native-editor.md), with language servers, Git signs, and an optional Vim mode. The default |
| `"neovim"` | A [Neovim GUI](neovim.md) drawn by btmux, with your config. Needs Neovim 0.12 or newer on the server's `PATH` |
| `"vscode"` | [VS Code](vscode.md) in btmux, with its color scheme and font     |
| `"pane"`   | The terminal: the pane's running Neovim, else `$EDITOR` in its shell |

With `"pane"`, btmux types into the terminal only when that is safe: if a
program other than Neovim or the shell is in the foreground, opening a file
shows an error and leaves that program alone.

## Over a pane

The editors and the [browser](browser.md) all behave the same way:

- They cover the active pane; other panes stay visible and usable.
- Zooming the pane (`<prefix> + z`) makes them fill the window.
- The same key, or Esc twice quickly, hides them. Pressed from another pane,
  the key moves them there.
- Every client sees the same editor. There is one Neovim, one VS Code, and
  one browser per server; the native editor can be open over any number of
  panes, each with its own files.

## Opening files from a shell

```sh
btmux open-editor src/main.rs 42
btmux open-editor --kind vscode README.md
```

`btmux open-editor [FILE] [LINE]` follows the `editor` setting from a pane,
and `--kind native|neovim|vscode|pane` overrides it. Outside btmux it runs `$EDITOR`.

## Using both

Whatever `editor` says, the status bar has a button for each editor the server
can run, with the configured one first. The `editor-native`, `editor-neovim`,
and `editor-vscode` actions toggle one directly, from the command palette or a
[`[keys]`](../configuration/keybindings.md) binding.

Neovim and VS Code are found on the server's `PATH`, else your shell's (a service's `PATH`
may be shorter), or for VS Code on macOS in `/Applications`.

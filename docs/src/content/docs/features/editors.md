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
| `"neovim"` | A [Neovim GUI](neovim.md) drawn by btmux, with your config. The default when Neovim 0.12 or newer is on the server's `PATH` |
| `"vscode"` | [VS Code](vscode.md) in btmux, with its color scheme and font     |
| `"pane"`   | The terminal: the pane's running Neovim, else `$EDITOR` in its shell. The default without Neovim 0.12 |

## Over a pane

The Neovim GUI, VS Code, and the [browser](browser.md) all behave the same
way:

- They cover the active pane; other panes stay visible and usable.
- Zooming the pane (`<prefix> + z`) makes them fill the window.
- The same key, or Esc twice quickly, hides them. Pressed from another pane,
  the key moves them there.
- There is one of each per server, so every client sees the same editor.

## Opening files from a shell

```sh
btmux open-editor src/main.rs 42
btmux open-editor --kind vscode README.md
```

`btmux open-editor [FILE] [LINE]` follows the `editor` setting from a pane,
and `--kind neovim|vscode|pane` overrides it. Outside btmux it runs `$EDITOR`.

## Using both

Whatever `editor` says, the status bar has a button for each editor the server
can run, with the configured one first. The `editor-neovim` and
`editor-vscode` actions toggle one directly, from the command palette or a
[`[keys]`](../configuration/keybindings.md) binding.

Editors are found on the server's `PATH`, else your shell's (a service's `PATH`
may be shorter), or for VS Code on macOS in `/Applications`.

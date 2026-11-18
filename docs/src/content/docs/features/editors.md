---
title: "Editors"
sidebar:
  order: 2
---

`<prefix> + e` opens the editor over the active pane. The
[file browser and Git mode](files-and-git.md) open files in the same editor.
Choose it with `editor` in [`config.toml`](../configuration/index.md).

| `editor`   | Opens                                                             |
| ---------- | ----------------------------------------------------------------- |
| `"native"` | btmux's own [editor](native-editor.md), with language servers, Git signs, and an optional Vim mode. The default |
| `"neovim"` | A [Neovim GUI](neovim.md) drawn by btmux, with your config. Needs Neovim 0.12 or newer on the server's `PATH` |
| `"vscode"` | [VS Code](vscode.md) in btmux, with its color scheme and font     |
| `"pane"`   | The terminal: the pane's running Neovim, else `$EDITOR` in its shell |

With `"pane"`, opening a file requires Neovim or the shell in the foreground.
Otherwise, btmux shows an error.

## Over a pane

Editors and the [browser](browser.md) share these controls.

- They cover the active pane; other panes stay visible and usable.
- Zooming the pane (`<prefix> + z`) makes them fill the window.
- The same key, or Esc twice quickly, hides them. Pressed from another pane,
  the key moves them there.
- Clients share one Neovim, one VS Code, and one browser per server. Each
  pane can have its own native editor, with files shared across clients.

## Capturing a pane

`<prefix> + [` (`capture-pane`) opens the pane's screen and scrollback in a
temporary file using the configured editor. The pane's program keeps running.
With `editor = "pane"` or a capture over the native editor's 16 MiB limit,
`$EDITOR` (or `vi`) opens in a [popup](../automation/popups.md).

A capture is deleted when you close its tab in the native editor, when its
popup closes, or when the server stops.

## Opening files from a shell

```sh
btmux open-editor src/main.rs 42
btmux open-editor --kind vscode README.md
```

`btmux open-editor [FILE] [LINE]` follows the `editor` setting from a pane,
and `--kind native|neovim|vscode|pane` overrides it. Outside btmux it runs `$EDITOR`.

## Using both

The status bar shows a button for the configured editor.
Use `editor-native`, `editor-neovim`, or `editor-vscode` to toggle one from
the command palette or a [`[keys]`](../configuration/keybindings.md) binding.

Put `nvim` on the server's `PATH`. See [VS Code server options](vscode.md#server)
for its executable locations.

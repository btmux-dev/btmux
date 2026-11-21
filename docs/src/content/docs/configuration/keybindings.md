---
title: "Key bindings"
sidebar:
  order: 2
---

Press and release the prefix, then press the bound key. Use
`<prefix> + ?` to see your current bindings.

```toml
prefix = "C-a"   # the default: Ctrl+A

[keys]
split-vertical = "|"
split-horizontal = "-"
resize-pane-left = "M-ArrowLeft"
```

A key is one character or a name such as `Space`, `Enter`, `Tab`, `Escape`,
`ArrowLeft`, or `F1`-`F12`, with optional `C-` (Ctrl) and `M-` (Alt)
modifiers. Setting a key replaces the action's default binding.

## Defaults

| Key                     | Action                                 |
| ----------------------- | -------------------------------------- |
| `%` `"`                 | `split-vertical`, `split-horizontal`   |
| Arrows                  | `navigate-left`, `-right`, `-up`, `-down` |
| `o` `;`                 | `next-pane`, `last-pane`               |
| `x` `z` `q`             | `kill-pane`, `zoom-pane`, `display-panes` |
| `{` `}`                 | `swap-pane-back`, `swap-pane-forward`  |
| `Space`                 | `next-layout`                          |
| `C-Arrow`               | `resize-pane-<direction>`, by 1 cell   |
| `M-Arrow`               | `resize-pane-<direction>-5`, by 5 cells |
| `PageUp` `PageDown`     | `scroll-page-up`, `scroll-page-down`   |
| `/` `[` `]`             | `search-pane`, `capture-pane`, `paste-clipboard` |
| `c` `n` `p`             | `new-window`, `next-window`, `prev-window` |
| `0`-`9`                 | Select a window (not rebindable)       |
| `,` `&`                 | `rename-window`, `kill-window`         |
| `w`                     | `window-grid`                          |
| `C` `s` `$`             | `new-session`, `choose-session`, `rename-session` |
| `(` `)` `L`             | `prev-session`, `next-session`, `last-session` |
| `W` `X`                 | `new-worktree`, `remove-worktree`      |
| `f` `g` `t`             | `file-browser`, `git-view`, `process-view` |
| `e` `b` `a`             | `editor`, `browser`, `agent-grid`      |
| `m`                     | `toggle-latex`                         |
| `?` `:`                 | `list-keys`, `command-palette`         |

`editor-native`, `editor-neovim`, `editor-vscode`, and
`toggle-key-overlay` (the [key overlay](../features/key-overlay.md)) are
unbound, as are the [native editor](../features/native-editor.md#actions)'s
`editor-*` actions. Every action is also in the command palette.

## VS Code keys

These work without the prefix, as in VS Code, in the pane or popup that has
focus. While VS Code has focus, they go to VS Code. The prefix comes first: a
`prefix = "C-,"` or `"C-g"` keeps its key, and a key bound after the prefix
runs its binding. They aren't rebindable; `vscode-keys = false` turns them
off, also in Settings.

```toml
vscode-keys = false
```

| macOS             | Windows and Linux | Action                                         |
| ----------------- | ----------------- | ---------------------------------------------- |
| `Cmd+Shift+P`     | `Ctrl+Shift+P`    | `command-palette`                              |
| `Cmd+P`           |                   | `editor-quick-open` (with `editor = "native"`) |
| `Cmd+Shift+E`     | `Ctrl+Shift+E`    | `file-browser`                                 |
| `Ctrl+Shift+G`    | `Ctrl+Shift+G`    | `git-view`                                     |
| ``Ctrl+Shift+` `` | ``Ctrl+Shift+` `` | `new-window`                                   |
| `Cmd+,`           | `Ctrl+,`          | Settings                                       |
| `Cmd+F`           |                   | `search-pane`, in a terminal                   |
| `Cmd+\`           | `Ctrl+Shift+5`    | `split-vertical`, in a terminal                |

Off macOS, VS Code's plain `Ctrl` keys are left to the shell: `Ctrl+P`,
`Ctrl+F`, and `Ctrl+K` are readline keys.

## Popup keys

Use one [`[[popup]]` table](../automation/popups.md#on-a-key) per command.
Each key follows the prefix and replaces any existing binding.

## Prefix twice

`<prefix> <prefix>` sends the prefix key itself to the pane, so `C-a C-a`
reaches a nested tmux or readline's beginning-of-line.

## Resizing

`resize-pane-<direction>` moves the pane's edge in that direction by one cell.
Any `resize-pane-<direction>-<cells>` name works as an action. macOS reserves
Ctrl+Arrow for Spaces, so rebind the one-cell keys there.

## Repeating keys

`repeat-time` sets how many milliseconds you can repeat a navigation, resize,
or page-scroll key without the prefix. Other keys end the repeat and go to
the terminal. The default is `0` (off).

```toml
repeat-time = 500
```

## Vi-style navigation

`h` `j` `k` `l` move between panes like the arrow keys, unless you bind those
keys to something else.

## macOS editing keys

On macOS, panes translate Cmd editing chords as Ghostty does:

| Keys                     | Sends                                      |
| ------------------------ | ------------------------------------------ |
| Cmd+Left, Cmd+Right      | `C-a`, `C-e` (line start, line end)        |
| Cmd+Backspace            | `C-u` (delete to line start)               |
| Cmd+Delete               | `C-k` (delete to line end)                 |
| Shift+Cmd+Left/Right     | The arrow with shift and super, for programs that select to the line edge |
| Option+Left/Right, Option+Backspace | Move and delete by word         |

Set `[terminal] macos-edit-keys = false` to leave Cmd chords to the browser.

## Key repeat on macOS

Holding a letter key on macOS opens the accent menu instead of repeating the
key, so holding `l` in Vim or `less` stops after one step. A web page can't
turn this off. The desktop app turns it off for itself; in a browser, turn it
off for that browser and restart it:

```sh
defaults write com.google.Chrome ApplePressAndHoldEnabled -bool false
```

Use `com.apple.Safari` for Safari. If that has no effect, turn it off for
every app with `defaults write -g ApplePressAndHoldEnabled -bool false`.

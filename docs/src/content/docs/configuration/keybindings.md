---
title: "Key bindings"
sidebar:
  order: 2
---

Key bindings start with a prefix, as in tmux: press it, release, then press
the key. Press `<prefix> + ?` in btmux for the live list.

```toml
prefix = "C-a"   # the default: Ctrl+A

[keys]
split-vertical = "|"
split-horizontal = "-"
resize-pane-left = "M-ArrowLeft"
```

A key is one character or a name such as `Space`, `Enter`, `Tab`, `Escape`,
`ArrowLeft`, or `F1`-`F12`, with optional `C-` (Ctrl) and `M-` (Alt)
modifiers. Each action has one key: setting it moves the action off its default
key.

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

`editor-native`, `editor-neovim`, and `editor-vscode` are unbound, as are the
[native editor](../features/native-editor.md#actions)'s `editor-*` actions.
Every action is also in the command palette.

## Popup keys

A `[[popup]]` table binds a key, after the prefix, that runs a command in a
[popup](../automation/popups.md) over the active pane. Repeat it for more
keys. The key is taken from any action bound to it.

```toml
[[popup]]
key = "G"
command = "lazygit"
title = "Git" # optional; the command by default
```

## Prefix twice

`<prefix> <prefix>` sends the prefix key itself to the pane, so `C-a C-a`
reaches a nested tmux or readline's beginning-of-line.

## Resizing

`resize-pane-<direction>` moves the pane's edge in that direction by one cell.
Any `resize-pane-<direction>-<cells>` name works as an action. macOS reserves
Ctrl+Arrow for Spaces, so rebind the one-cell keys there.

## Repeating keys

With `repeat-time` set, navigation, resize, and page-scroll keys repeat without
the prefix for that many milliseconds. Any other key ends the repeat and goes
to the terminal. The default is `0`, which turns it off; try `500`.

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

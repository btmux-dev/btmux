---
title: "tmux-style commands"
sidebar:
  order: 2
---

`btmux` supports a subset of tmux commands, including familiar aliases and
flags. Commands target the current pane's server or, outside btmux, the
running server for `--profile`. See [All commands](../configuration/commands.md).

```sh
btmux list-panes -F '#{pane_index} #{pane_id} #{pane_current_path}'
pane=$(btmux split-window -h -d -c "$PWD")
btmux send-keys -t "$pane" 'npm test' Enter
btmux capture-pane -p -t "$pane" -S -200
btmux kill-pane -t "$pane"
```

| Command                      | Flags                            |
| ---------------------------- | -------------------------------- |
| `list-sessions` (`ls`)       | `-F`                             |
| `list-windows` (`lsw`)       | `-a -t -F`                       |
| `list-panes` (`lsp`)         | `-a -s -t -F`                    |
| `new-session` (`new`)        | `-s -n -c -F`                    |
| `new-window` (`neww`)        | `-t -n -c -d -F`                 |
| `split-window` (`splitw`)    | `-t -h -v -b -d -c -F`           |
| `kill-pane` (`killp`)        | `-t`                             |
| `kill-window` (`killw`)      | `-t`                             |
| `kill-session`               | `-t`                             |
| `select-pane` (`selectp`)    | `-t`                             |
| `select-window` (`selectw`)  | `-t`                             |
| `rename-window` (`renamew`)  | `-t NAME`                        |
| `rename-session` (`rename`)  | `-t NAME`                        |
| `send-keys` (`send`)         | `-t -l KEY...`                   |
| `capture-pane` (`capturep`)  | `-t -S` (`-p`, `-J` are implied) |
| `new-worktree`               | `-t -d -F BRANCH`                |
| `remove-worktree`            | `-t`                             |
| `display-popup` (`popup`)    | `-t -d -e -T -E`; see [Popups](popups.md) |

Coding agents learn these commands from the btmux skill; see
[Agent skill](../agents/index.md#agent-skill).

## Differences from tmux

- `-t` takes a pane, window, or session ID, or `SESSION[:WINDOW[.PANE]]` by
  name or index. An empty part is the current one (`:1`, `:.2`, `work:`).
  Without `-t`, the target is the pane the command runs in (`BTMUX_PANE_ID`).
  Pane indexes follow the layout, as `<prefix> + q` shows them.
- `new-session`, `new-window`, and `split-window` print the new pane's ID, as
  tmux does with `-P -F '#{pane_id}'`. Sessions are always detached.
- List commands print JSON unless given `-F`. Formats only substitute
  `#{name}`: the `session_`, `window_`, and `pane_` fields of the JSON, plus
  `session_windows` and `window_panes`. Flags print as `1` or `0`.
- `send-keys` sends each argument as a key name (`Enter`, `C-c`, `M-x`, `Up`,
  `F5`, ...) if it is one, else as text, without spaces between arguments.
  `-l` sends them all as text.
- `capture-pane` always prints, with wrapped lines joined and no escapes.
  `-S -` includes all retained history and `-S -N` the last N rows of it.
- `kill-pane` closes the window with its last pane, and the session with its
  last window, but never the last session.

## Worktrees

`new-worktree` and `remove-worktree` follow the same rules as the
[worktree key bindings](../features/files-and-git.md#worktrees).

- `new-worktree BRANCH` adds a Git worktree for the target pane's repository
  and opens a window there, in the pane's session. It prints the new pane's
  ID.
- `remove-worktree` removes the worktree the target pane is in and closes
  every pane in it. It prints the worktree's path.

## Shell completions

`btmux completions <shell>` prints a completion script for bash, fish, zsh,
PowerShell, or Elvish. Save it in your shell's completion directory:

```sh
# fish
btmux completions fish > ~/.config/fish/completions/btmux.fish

# zsh: add ~/.zfunc to fpath before compinit runs in ~/.zshrc
mkdir -p ~/.zfunc && btmux completions zsh > ~/.zfunc/_btmux

# bash
mkdir -p ~/.local/share/bash-completion/completions
btmux completions bash > ~/.local/share/bash-completion/completions/btmux
```

```powershell
# PowerShell: load completions from your profile
Add-Content $PROFILE 'btmux completions powershell | Out-String | Invoke-Expression'
```

Regenerate the script after updating btmux. The
[Home Manager module](../start/installation.mdx#nix) installs completions itself.

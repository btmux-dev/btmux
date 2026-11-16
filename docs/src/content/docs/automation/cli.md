---
title: "tmux-style commands"
sidebar:
  order: 2
---

The `btmux` binary has a subset of tmux's commands, with the same names,
aliases, and main flags, for scripts and agents. Inside a pane they target that
pane's server; otherwise the running server of `--profile`.

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

For coding agents, [`skills/btmux`](../../../../../skills/btmux/SKILL.md) is an
[Agent Skill](https://agentskills.io) that teaches these commands. Install it
with `npx skills add buntec/btmux`, or copy the folder into your agent's skills
directory (`~/.claude/skills/` for Claude Code).

Differences from tmux:

- `-t` takes a pane, window, or session ID, or `SESSION[:WINDOW[.PANE]]` by
  name or index. An empty part is the current one (`:1`, `:.2`, `work:`).
  Without `-t`, the target is the pane the command runs in (`BTMUX_PANE_ID`).
  Pane indexes follow the layout, as `prefix + q` shows them.
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

`new-worktree` and `remove-worktree` aren't tmux commands. `new-worktree
BRANCH` adds a git worktree for the target pane's repo, beside its main
worktree (`../<repo>-<branch>`, or the `worktree-path` setting), and opens a
window there in the pane's session, printing its pane ID. An existing local
branch, or a remote one (as a tracking branch), is checked out; a new branch
starts at the target pane's HEAD. `remove-worktree` removes the linked
worktree the target pane is in and closes every pane in it, printing its path.
It refuses the main worktree, uncommitted changes, a locked worktree, and panes
running a program, and keeps the branch. `prefix + W` and `prefix + X` do the
same in the browser.

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

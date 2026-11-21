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
| `new-window` (`neww`)        | `-t -n -c -d -F -e --keep-open --wait [COMMAND]` |
| `split-window` (`splitw`)    | `-t -h -v -b -d -c -F -e --keep-open --wait [COMMAND]` |
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
| `wait-exit`                  | `-t --timeout`                   |
| `display-popup` (`popup`)    | `-t -d -e -T -E`; see [Popups](popups.md) |

Coding agents learn these commands from the btmux skill; see
[agent setup](../agents/index.md#setup).

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

## Running a command and waiting for it

Scripts and agents can run a command in its own pane and get its exit
status, unlike typing it with `send-keys`, which can't tell when it finished.

```sh
btmux new-window -d -n tests --wait -- cargo test   # exits with the tests' status
pane=$(btmux split-window -h -d --keep-open 'npm run build')
btmux wait-exit -t "$pane" --timeout 600            # its status; 124 on timeout
btmux capture-pane -p -t "$pane" -S -               # its output, after it exits
btmux kill-pane -t "$pane"
```

- `new-window` and `split-window` run COMMAND, the arguments after the flags,
  instead of the shell. A single argument is a command line for the shell;
  several run directly. It starts at once, whether or not a client shows the
  pane, in the current directory unless `-c` says otherwise, with this
  shell's environment plus any `-e NAME=VALUE`, as [popups](popups.md) run.
  A command that can't start is an error, and nothing changes.
- In a [sandboxed session](../features/sandbox.md), the command runs in the
  guest, in the directory asked for, with the guest's environment rather than
  this shell's.
- The pane closes when its program exits. With `--keep-open` it stays, with
  the output and an `[exited with status N]` line, until you close it; typing
  into it does nothing. `list-panes` shows `pane_dead` and `pane_dead_status`
  for it.
- `--wait` prints the pane's ID, then waits for the program to exit and exits
  with its status, as `wait-exit` does.
- `wait-exit` waits for the target pane's program (its command, or its shell)
  to exit, and exits with its status. A signal N gives 128 + N, and statuses
  above 255 are reduced modulo 256. It exits 1 if the pane closed before its
  program reported a status (`kill-pane`), and 124 if `--timeout` seconds pass
  first.
- For a kept pane, `wait-exit` returns once the output has ended and the
  `[exited ...]` line is written, so `capture-pane` right after it sees all
  of it. If something else, such as a background job, holds the terminal
  open, that's at most 2 seconds after the program exits.
- A pane that closed in the last 5 minutes still reports its status, so `-t`
  takes a closed pane's ID; the server keeps the last 256 of them. A wait
  already running when its pane closes always gets the status. Waits
  continue across `btmux upgrade`, and a kept pane keeps its output and
  status through one.

`btmux wait` is a different command: it waits for an
[agent's state](../agents/orchestrating.md).

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

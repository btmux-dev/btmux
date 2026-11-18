---
title: "btmux for tmux users"
sidebar:
  order: 5
---

Most default keys match tmux. The default prefix is `C-a`, the most common tmux
remap; set `prefix = "C-b"` for stock tmux. This page covers where btmux
differs.

## Same as tmux

| Keys                   | Action                              |
| ---------------------- | ----------------------------------- |
| `%` `"`                | Split                               |
| Arrows, `o`, `;`       | Move between panes, next, last      |
| `x` `z` `q`            | Kill pane, zoom, pane numbers       |
| `{` `}` `Space`        | Swap pane, next layout              |
| `c` `n` `p` `0`-`9`    | New window, next, previous, select  |
| `,` `&`                | Rename window, kill window          |
| `$` `(` `)` `L`        | Rename session, previous, next, last |
| `?`                    | Key list                            |

## Same key, different meaning

| Key     | tmux        | btmux                                             |
| ------- | ----------- | ------------------------------------------------- |
| `w`     | choose-tree | window grid (live thumbnails)                     |
| `s`     | choose-tree | session/window switcher                           |
| `t`     | clock       | process viewer                                    |
| `f`     | find-window | file browser                                      |
| `m`     | mark pane   | LaTeX overlay                                     |
| `[`     | copy mode   | opens the scrollback in `$EDITOR` inside the pane |
| `l`     | last-window | pane right (`h` `j` `k` too); last-window unbound |
| `/`     | describe key | search the scrollback                            |
| `]`     | paste buffer | pastes the system clipboard                      |
| `:`     | command prompt | command palette of btmux commands              |
| `C`     | (unbound)   | new session (prompts for name)                    |
| `g` `a` | (unbound)   | Git mode, agent view                              |
| `e` `b` | (unbound)   | editor, browser                                   |
| `W` `X` | (unbound)   | new Git worktree (prompts for branch), remove it  |

Rebind any of them under [`[keys]`](../configuration/keybindings.md).

## Behaviors that differ

- **Window order.** Windows are numbered by display order, which defaults to
  alphabetical (`window-sort`). `n`, `p` and `0`-`9` follow that order, not
  creation order. Set `window-sort = "created"` for tmux numbering.
- **Sessions are per client.** Two clients can show different sessions, while
  window and pane selection is shared by all of them. `L` goes back to that
  client's previous session.
- **No attach or detach.** Closing the client is the detach; the server keeps
  the shells running. After a server restart the layout returns but programs
  and scrollback do not; `btmux upgrade` keeps both.
- **Size owner.** The first client on a pane sets its size until it leaves or
  another client types, pastes, or clicks in it. tmux sizes to the smallest
  client by default.
- **Last pane, window, session.** Killing the last pane closes its window;
  killing the last window kills the session. The last session cannot be killed,
  and exiting its last shell starts a fresh session "0" instead of ending the
  server.
- **Paste.** A browser may ask for clipboard permission the first time.
- **Prefix timeout.** The prefix expires after 2 seconds; tmux waits
  indefinitely.
- **Repeat.** `repeat-time` defaults to `0` (off); tmux's default is 500 ms.
- **Browser keys.** Function keys and Cmd/Meta combinations are not forwarded
  to the terminal, apart from the
  [macOS editing keys](../configuration/keybindings.md#macos-editing-keys).
- **`[keys]`.** Each action has one key. Overriding it moves the action off its
  default key rather than adding a second one.
- **Popups.** `btmux display-popup` (or `popup`) takes `-t -d -e -T -E` and
  always closes when its program exits. Several arguments run as a program
  rather than a shell command. Bind one to a key with `[[popup]]` in
  `config.toml`, where tmux uses `bind-key ... display-popup -E`; see
  [Popups](../automation/popups.md).

## Not available

- Copy mode and paste buffers (`=`, `#`). Select with the mouse; btmux has no
  buffer stack.
- tmux commands in `:`. The prompt is a palette of btmux commands, such as
  `select-layout-*`, not `split-window -h` or `resize-pane -R 5`. From a shell,
  the [`btmux` command](../automation/cli.md) has a subset of tmux's.
- `d` detach, `!` break-pane, `.` move-window, `'` select window by prompt,
  `i` display message, `r` refresh, `C-o` rotate, `M-1`..`M-5` layouts.
- `bind-key`, `set-option` and a `.tmux.conf`; configuration is `config.toml`.

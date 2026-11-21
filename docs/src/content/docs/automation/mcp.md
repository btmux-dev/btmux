---
title: "MCP server"
sidebar:
  order: 4
---

Connect to `/mcp` to manage sessions, panes, and
[agents](../agents/orchestrating.md). Use `create_worktree` and
`remove_worktree` to manage Git worktrees. `system_info` describes the host
like `GET /api/system`. `read_pane_output` returns the rendered screen and
takes `scrollback` and `lines` like `/capture`. `send_keys` takes `text` and
tmux-style `keys`.

For the [native editor](../features/native-editor.md): `open_in_editor` opens
a file over a pane, optionally at a line, and starts its language servers;
`editor_buffers` lists the open files and their unsaved changes; and
`editor_diagnostics` returns what the language servers report for them
(1-based lines), optionally only under a path.

`run_command` returns ANSI-stripped output after 400 ms of silence by
default, even if the process is still running. The overall timeout defaults
to 15 seconds. Set either interval or check again with `read_pane_output`.

To know when a command finished, and how, give `create_window` or
`split_pane` a `command`: the new pane runs that command line instead of the
shell, starting at once, and closes when it exits unless `keep_open` is set.
`wait_for_exit` then returns its exit status (128 + N for signal N; `null` if
the pane closed before it reported one), or `timed_out` after `timeout_ms`
(10 minutes by default). For a kept pane it returns once the output has
ended, so `read_pane_output` after it sees all of it. It works for any
pane's program, including its shell, and for a pane closed in the last 5
minutes (the last 256 of them). In a sandboxed session the command runs in
the guest.

Register the server with Claude Code:

```sh
claude mcp add --transport http btmux http://127.0.0.1:8004/mcp \
  --header "Authorization: Bearer ${BTMUX_AUTH_TOKEN}"
claude mcp list
```

Add `--scope user` to the first command to make it available in every project.
The btmux server must already be running. Re-register the server if you change
its port or token.

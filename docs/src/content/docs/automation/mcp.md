---
title: "MCP server"
sidebar:
  order: 4
---

The MCP server at `/mcp` exposes tools for creating and inspecting sessions,
splitting panes, creating and removing Git worktrees (`create_worktree`,
`remove_worktree`), running commands, sending text and keys, reading pane
output, describing the host (`system_info`, like `GET /api/system`), and
[driving agents](../agents/orchestrating.md). `read_pane_output` returns the
rendered screen (with `scrollback` and `lines` like `/capture`), and
`send_keys` takes `text` and tmux-style `keys`.

For the [native editor](../features/native-editor.md): `open_in_editor` opens
a file over a pane, optionally at a line, and starts its language servers;
`editor_buffers` lists the open files and their unsaved changes; and
`editor_diagnostics` returns what the language servers report for them
(1-based lines), optionally only under a path.

`run_command` waits until output has been idle for 400 ms by default, then
returns an ANSI-stripped result. This does not prove that the process exited: a
quiet, long-running command can return early. The overall timeout defaults to
15 seconds, and callers can adjust both intervals or follow up with
`read_pane_output`.

Register the server with Claude Code:

```sh
claude mcp add --transport http btmux http://127.0.0.1:8004/mcp \
  --header "Authorization: Bearer ${BTMUX_AUTH_TOKEN}"
claude mcp list
```

Add `--scope user` to the first command to make it available in every project.
The btmux server must already be running. Re-register the server if you change
its port or token.

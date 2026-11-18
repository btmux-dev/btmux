---
title: "Popups"
sidebar:
  order: 6
---

`btmux popup` (alias `display-popup`) runs a program in a terminal over a
pane. The popup closes when the program exits and returns its exit status.

```sh
btmux popup lazygit
btmux popup -T Log git log --oneline --graph
btmux popup 'git diff | delta'
```

Several arguments are a program and its arguments, run as given. A single
argument is a command line for the configured shell.

| Flag            | Effect                                                    |
| --------------- | --------------------------------------------------------- |
| `-d DIR`        | Run in DIR instead of the command's directory             |
| `-e NAME=VALUE` | Add to the program's environment, which is the command's  |
| `-T TITLE`      | The title, the command by default                         |
| `-t TARGET`     | Open over another pane, named as for the [tmux-style commands](cli.md) |

`-E` is accepted for tmux scripts; popups always close when the program exits.
tmux's size, position, and style flags (`-w`, `-h`, `-x`, `-y`, `-b`, `-B`,
`-s`, `-S`, `-c`) are accepted and ignored.

## On a key

Add a `[[popup]]` table to `config.toml`. The key follows the prefix and
replaces any existing binding. The command runs in the pane's directory
through an interactive shell with the pane's `PATH`.

```toml
[[popup]]
key = "G"
command = "lazygit"
title = "Git" # optional; the command by default
```

## Behavior

- Every key goes to the program, Esc included, except the prefix: prefix
  bindings still work, and `<prefix> <prefix>` sends the prefix key to the
  program.
- A popup belongs to its pane. Switching windows hides it while it keeps
  running, it outlives reloads and `btmux upgrade`, and closing the pane
  closes it. A pane has one popup at a time.
- The close button in its title bar hangs the program up (status 129).
- [`btmux pick`, `confirm`, and `input`](widgets.md) run in a popup show in
  it, and pane search (`<prefix> + /`) and clipboard pastes go to it.
- Popups of [sandboxed](../features/sandbox.md) panes run in the sandbox, in
  its login shell's environment.
- A popup opened while a dialog such as Settings or the file browser is open
  waits for it to close.
- The program runs in the current terminal outside btmux or if no client
  shows the pane within 3 s. If it can't start, the command exits 127.

## API

`POST /api/panes/<pane-id>/popup` opens one, with `command` either
`{"exec": ["program", "arg"]}` or `{"shell": "command line"}`, and optional
`cwd` (the pane's directory by default), `env` (a list of `[name, value]` pairs
that replaces the server's environment), `title`, and `id`, a UUID the caller
picks so a retried request finds its popup instead of running the program
again.

It replies `{"id": ...}` once a client has started the program, 409 if none does
within 3 s, 422 if the program can't start, and 423 if the pane already has a
popup. `GET /api/panes/<pane-id>/popup/<id>` waits for the program to exit and
replies `{"status": n}` (`null` if unknown). Exit statuses stay available for 5
minutes, so a waiter can reconnect.

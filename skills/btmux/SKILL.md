---
name: btmux
description: Control btmux, a browser-based tmux, from the shell. Use when running inside btmux ($BTMUX_PANE_ID is set) or when asked to work with btmux sessions, windows, or panes, for example to run a dev server or tests in a separate pane, read another pane's output, notify the user, or start and coordinate other coding agents.
---

# btmux

btmux is a tmux-like terminal multiplexer whose panes the user sees in a
browser. The `btmux` CLI talks to the running server. Inside a btmux pane,
`$BTMUX_PANE_ID` is your pane's ID and commands reach this server without
setup; outside one, they use the default server (or `--profile NAME`). If
`btmux` isn't on `PATH`, `"$BTMUX_BIN"` is the server's own binary.

The commands mirror tmux, so use what you know from tmux, except for the
differences below. `btmux --help` lists the commands and
`btmux <command> --help` each one's flags.

## Where you are

The context at the start of your session may name your session, window, and
pane. It comes from your environment and wasn't checked since; if a target
isn't found, look again:

```sh
btmux list-panes -a -F '#{pane_id} #{session_name}:#{window_index}.#{pane_index} #{pane_active}' |
  grep "$BTMUX_PANE_ID"
```

That is where you run, not necessarily what the user is looking at.

- **"Here", "beside you", or no place named:** your pane, the default target.
- **"The current window" or "the pane I'm in":** the active window of the
  session (`#{window_active}`) and its active pane (`#{pane_active}`).
- **A session, window, or pane by name:** find it in a listing; it wins over
  where you run.

## Panes, windows, sessions

```sh
btmux list-panes -F '#{pane_index} #{pane_id} #{pane_active} #{pane_current_path}'
btmux list-panes -a          # every pane, as JSON
btmux list-windows -a -F '#{session_name}:#{window_index} #{window_name}'
btmux list-sessions
```

`-t` takes a pane, window, or session ID, or `SESSION[:WINDOW[.PANE]]` by name
or index (`work:1.0`, `:2` for window 2 of this session, `:.1` for pane 1 of
this window). Without `-t`, the target is your own pane. Prefer pane IDs: they
stay valid when panes move or other panes close.

`new-session`, `new-window`, and `split-window` print the new pane's ID:

```sh
pane=$(btmux split-window -h -d -c "$PWD")     # side by side, keep focus here
win=$(btmux new-window -d -n server -c "$PWD") # background window
btmux kill-pane -t "$pane"
```

Pass `-d` so you don't move the user's focus, and `-c "$PWD"` so the shell
starts in your directory. The last pane of a window closes the window, and the
last window closes the session.

Other commands: `kill-window`, `kill-session`, `select-pane`, `select-window`,
`rename-window`, `rename-session`.

To work on a branch in its own git worktree, with a window there:

```sh
pane=$(btmux new-worktree -d feat/login)  # beside the repo: ../<repo>-feat-login
btmux send-keys -t "$pane" 'npm test' Enter
btmux remove-worktree -t "$pane"          # closes its panes; keeps the branch
```

`new-worktree` checks out an existing branch or creates one from your HEAD.
`remove-worktree` refuses uncommitted changes and panes running a program.

## Typing and reading

```sh
btmux send-keys -t "$pane" 'npm run dev' Enter
btmux send-keys -t "$pane" C-c
btmux send-keys -t "$pane" -l 'text sent as is, even Enter'
btmux capture-pane -p -t "$pane"            # the visible screen
btmux capture-pane -p -t "$pane" -S -200    # plus 200 rows of history
btmux capture-pane -p -t "$pane" -S -       # all retained history
```

Each `send-keys` argument is a key name (`Enter`, `Tab`, `Escape`, `C-c`,
`M-x`, `Up`, `PageDown`, `F5`) if it is one, else literal text; arguments are
joined without spaces and nothing adds Enter. `capture-pane` prints plain text
with wrapped lines joined. Output arrives asynchronously: after sending a
command, poll `capture-pane` until its result or the next prompt appears rather
than assuming it finished.

## Watching for changes

```sh
btmux events                      # one line per event, until interrupted
btmux events --json | jq -c 'select(.type == "agent_status_changed")'
btmux events -s work --json       # one session; exits once it closes
```

Events cover sessions, windows, and panes created, renamed, selected, moved,
or closed, pane titles and directories, agent state changes, notifications,
and widgets and popups opening and closing. With `--json`, each line has
`type`, `time` (Unix milliseconds), and the `session_id`, `window_id`, and
`pane_id` it is about, with names; the first is `ready`. It reconnects after
the server restarts or upgrades, printing `ready` again; changes in between
are missed, so list what you track again then. Run it in the background and
stop it when you are done.

## Asking the user

These open over your pane in the browser and print the answer:

```sh
choice=$(printf 'main\ndevelop\nrelease\tthe next release\n' | btmux pick --prompt 'Branch')
btmux confirm 'Delete the build directory?' && rm -rf build
name=$(btmux input --prompt 'Window name' --default server)
btmux popup -T 'Review' -- git add -p    # a program over the pane; exits with its status
```

`pick` items are stdin's lines; a tab starts a description that isn't printed
(`-m` picks several). `confirm` exits 0 for yes and 1 for no. `pick`,
`confirm`, and `input` exit 130 if the user cancels and 2 if no browser tab
shows the pane; ask in your reply instead then. `popup` runs the program in
place when no tab shows the pane. Ask this way only when a choice in the
browser beats asking in the conversation, such as picking from a long list.

## Opening files

```sh
btmux open-editor src/main.rs 120   # over your pane in the user's editor, at line 120
```

## Other agents

```sh
btmux agents                          # panes running an agent, with their states, as JSON
btmux wait "$pane"                    # until it is idle, done, or blocked
btmux wait "$pane" --state done --timeout 1800
```

States are `working`, `waiting` (its turn ended, but background work will
resume it), `blocked` (waiting for approval or input), `done` (finished a
turn), `idle`, and `unknown`. `--timeout` is in seconds (default
600, at most 3600); `btmux wait` exits 2 on timeout.

To give an agent a prompt, use `btmux prompt` instead of `send-keys`: it
pastes the text, presses Enter reliably, waits for the turn to finish, and
prints the agent's state and rendered screen as JSON. It exits 2 on timeout.
The status `message` is only the first line of the agent's last reply; read
`screen`, or have the agent write longer results to a file.

```sh
btmux prompt "$pane" 'Run the tests and fix any failures.'
btmux prompt "$pane" < task.md                 # prompt from stdin
btmux prompt "$pane" --no-wait 'Also update the docs.'
```

It refuses while the agent is `blocked` (read the pane and answer with
`send-keys`) and, unless `--no-wait`, while it is `working`.

To start an agent, launch it in a new pane with `send-keys`, then `btmux wait`
for it before prompting. To stop one, kill its pane.

## Notifying the user

```sh
btmux send-notification 'Tests passed' --body '42 passed' --level success
```

Levels are `info`, `attention`, `success`, and `error`.

## MCP and REST

The same server offers an MCP server at `$BTMUX_API_URL/mcp` and a REST API
described by `$BTMUX_API_URL/api/openapi.json`, both taking
`Authorization: Bearer $BTMUX_AUTH_TOKEN`. Prefer the CLI from a shell; use
MCP tools (`list_sessions`, `send_keys`, `read_pane_output`, `run_command`,
`wait_for_agent`, `send_prompt`, ...) when they are already connected.

## Etiquette

- The panes are the user's. Don't type into, kill, or rename panes you didn't
  create unless asked; read them freely.
- Clean up panes and windows you created when you are done with them, unless
  the user wants them kept (for example a running dev server).
- Never print or log `$BTMUX_AUTH_TOKEN`.

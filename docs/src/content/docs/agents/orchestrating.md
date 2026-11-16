---
title: "Orchestrating agents"
sidebar:
  order: 3
---

btmux knows when each agent in a pane is working, blocked, done, or idle (see
[States](index.md#states)), so scripts and agents can wait for that state
instead of scraping the screen or polling.

| MCP tool         | REST                               | Purpose                                                       |
| ---------------- | ---------------------------------- | ------------------------------------------------------------- |
| `list_agents`    | `GET /api/agents`                  | Panes running an agent, with session, window, cwd, and status |
| `wait_for_agent` | `GET /api/panes/<pane-id>/wait`    | Wait until the agent reaches one of `states`, or exits        |
| `send_prompt`    | `POST /api/panes/<pane-id>/prompt` | Paste a prompt, press Enter, and wait for the turn to finish  |

`wait_for_agent` waits for `idle`, `done`, or `blocked` by default (`states`;
comma-separated over REST) and returns `{status, timed_out}`. It returns at
once if the agent is already in such a state, and `status` is null once the
agent has exited. A pane with no agent yet is waited on, so it can follow
launching one. The timeout defaults to 10 minutes (`timeout_ms`, at most an
hour).

`send_prompt` pastes the prompt with bracketed paste, so it may span lines,
waits 300 ms (agent input boxes can drop an Enter that arrives with the text),
and presses Enter. With `wait` (the default), it waits up to 15 s for the turn
to start and then for the agent to settle, and returns `{status, started,
timed_out, screen}`, where `screen` is the rendered screen once output has
paused (hooks report before the agent finishes drawing), or null if the pane
closed. With `wait: false`, it
returns `{submitted: true}` once Enter is sent. It refuses while
the agent is `blocked`: typed text would answer the dialog, which `send_keys`
can do deliberately. With `wait`, it also refuses while the agent is
`working`, since the turn that ends next would not be this prompt's.

For an agent detected from its screen, it rechecks the screen itself before
pasting and again before the Enter, since a status can lag a dialog by a
settling period. It refuses when a dialog is visible, when the agent's state
is `unknown` (unless its rules can't recognize an idle prompt: currently
Gemini CLI and OpenCode), or when detection has stalled. If the check before
the Enter fails, or the agent was replaced meanwhile, the error says the
prompt was pasted but not submitted; the text stays in the agent's input.
This narrows the race but can't close it: a dialog can still appear between
the last check and the Enter.
Prompts to one pane are submitted one at a time, and a waited prompt first
waits (up to 15 s) for earlier prompts' turns to start, so it then finds the
agent `working` instead of claiming another prompt's turn. It can't tell its
turn from one started by a prompt typed in a browser, or by one queued with
`wait: false` while the agent was working, which may run after the current
turn.

`wait_for_agent` reports the current state, so right after a `wait: false`
prompt it can return before the turn starts. Wait for `working` first, or use
`send_prompt` with `wait`.

```sh
curl -H "Authorization: Bearer $BTMUX_AUTH_TOKEN" -X POST "$BTMUX_API_URL/api/panes/<pane-id>/prompt" \
  -H 'Content-Type: application/json' \
  -d '{"prompt":"Run the tests and fix any failures."}'
```

Over REST, `input`, `capture`, `wait`, and `prompt` return 400 for a bad
request, 404 for an unknown pane, 409 when the pane's state doesn't allow it
now (such as a busy agent), and 500 otherwise.

From a shell, `btmux agents` prints the agent list as JSON, `btmux wait`
blocks until an agent settles, then prints its status, and `btmux prompt`
submits a prompt (an argument or stdin) like `send_prompt`, printing its
outcome (`--no-wait` to only submit). `wait` and `prompt` exit 2 on timeout:

```sh
btmux wait <pane-id> --state done --state blocked --timeout 1800
btmux prompt <pane-id> 'Run the tests and fix any failures.'
```

Like `send-notification`, they target the pane's server inside btmux, and
otherwise the running server of `--profile`.

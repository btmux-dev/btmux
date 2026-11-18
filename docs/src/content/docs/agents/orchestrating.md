---
title: "Orchestrating agents"
sidebar:
  order: 3
---

Scripts and agents can submit prompts and wait for an agent to reach a
[reported state](index.md#states).

## An example

Start an agent in a worktree, wait for it to go idle, then submit a task.

```sh
pane=$(btmux new-worktree fix-flaky-tests)   # a worktree and a window; prints the pane
btmux send-keys -t "$pane" 'btmux claude' Enter
btmux wait "$pane" --state idle              # until the agent is ready
btmux prompt "$pane" 'Find and fix the flaky test in tests/server.test.ts.'
```

`btmux prompt` returns when the turn ends, printing the agent's status and
final screen as JSON. Run several of these in the background to fan work out,
then `btmux agents` to see where each one stands.

## The three operations

| Shell          | MCP tool         | REST                               | Does                                                    |
| -------------- | ---------------- | ---------------------------------- | ------------------------------------------------------- |
| `btmux agents` | `list_agents`    | `GET /api/agents`                  | Lists panes running an agent, with session, window, directory, and state |
| `btmux wait`   | `wait_for_agent` | `GET /api/panes/<pane-id>/wait`    | Waits until the agent reaches a state, or exits         |
| `btmux prompt` | `send_prompt`    | `POST /api/panes/<pane-id>/prompt` | Submits a prompt and waits for the turn to finish       |

Inside a pane the commands talk to that pane's server; elsewhere, to the
running server of `--profile`.

## Waiting

```sh
btmux wait <pane-id> --state done --state blocked --timeout 1800
```

- By default it waits for `idle`, `done`, or `blocked`, for up to 10 minutes
  (`--timeout` in seconds, `timeout_ms` over MCP and REST; at most an hour).
- It returns at once if the agent is already in such a state.
- It returns `{status, timed_out}`. `status` is null once the agent has
  exited. The command exits 2 on a timeout.
- You can start waiting before an agent appears in the pane.

## Prompting

```sh
btmux prompt <pane-id> 'Run the tests and fix any failures.'
git diff | btmux prompt <pane-id>            # the prompt from stdin
btmux prompt --no-wait <pane-id> 'Continue.' # submit and return
```

`prompt` pastes multiline text and pauses before sending Enter so the agent's
input box has time to process the paste.

By default it then waits for the turn to start (up to 15 s) and to finish, and
returns `{status, started, timed_out, screen}`. `screen` is the rendered screen
once output has paused, or null if the pane closed. With `--no-wait`
(`wait: false`) it returns `{submitted: true}` once Enter is sent.

### When it refuses

- **The agent is blocked.** Typed text would answer its dialog. To answer one
  on purpose, use `send-keys`.
- **The agent is working** and the caller requested a wait. The running
  turn would finish first.
- **A dialog is visible, or the state is unknown.** For agents tracked by
  their screen, btmux checks the screen again right before pasting and before
  Enter. Gemini CLI and OpenCode are exempt from the unknown-state rule, since
  their idle prompt can't be recognized.

If the check before Enter fails, or the agent was replaced meanwhile, the error
says the prompt was pasted but not submitted. The text stays in the agent's
input.

### What it can't guarantee

- A dialog can still appear between the last check and the Enter.
- Prompts to one pane are submitted one at a time, and a waited prompt first
  waits for earlier ones to start. But it can't tell its turn from one started
  by a person typing in the pane, or by a `--no-wait` prompt queued while the
  agent was working.
- Right after a `--no-wait` prompt, `wait` can return before the turn has
  started, since it reports the current state. Wait for `working` first, or
  let `prompt` do the waiting.

## Over REST

```sh
curl -H "Authorization: Bearer $BTMUX_AUTH_TOKEN" \
  -X POST "$BTMUX_API_URL/api/panes/<pane-id>/prompt" \
  -H 'Content-Type: application/json' \
  -d '{"prompt":"Run the tests and fix any failures."}'
```

`wait` takes `states` as a comma-separated query parameter. `input`,
`capture`, `wait`, and `prompt` return 400 for a bad request, 404 for an
unknown pane, 409 when the pane's state doesn't allow it now (such as a busy
agent), and 500 otherwise.

---
title: "Coding agents"
sidebar:
  order: 1
  label: "Overview"
---

btmux tracks Claude Code, Codex, and Gemini CLI in your panes so you can see
which agents need attention.

<btmux-demo name="agents" label="Demo: three agents in split panes, approving a permission request from the agent view, and jumping to a finished agent"></btmux-demo>

## Setup

Run this once to set up every agent btmux finds (Claude Code, Codex, and
Gemini CLI):

```sh
btmux skill setup
```

It installs, for each agent:

- **Status hooks**, which report the agent's state to btmux for more reliable
  status, notifications, and completion messages than detection by process
  and screen alone.
- **A skill**, for Claude Code and Codex, that teaches the agent to use btmux:
  to open windows and panes, run commands there and read their output, ask
  you with [widgets](../automation/widgets.md), and coordinate other agents.
  It goes in `~/.claude/skills/btmux` for Claude Code (under
  `CLAUDE_CONFIG_DIR` if set) and `~/.agents/skills/btmux` for Codex and
  other agents, and tells the agent to read the guide `btmux skill show`
  prints.
- **A context hook**, for Claude Code and Codex, that tells the agent at the
  start of each session which btmux session, window, and pane it runs in.

The skill and the context hook get their text from the installed `btmux` when
the agent uses them, so upgrading btmux updates what agents are told. Hooks
are merged into the agent's settings, leaving your own hooks alone. Running
setup again is safe and updates what an older btmux installed.

| Command               | Does                                         |
| --------------------- | -------------------------------------------- |
| `btmux skill setup`   | Install or update everything above           |
| `btmux skill status`  | Show what is installed for each agent        |
| `btmux skill remove`  | Remove it, leaving your other hooks alone    |
| `btmux skill show`    | Print the guide                              |

`setup` and `remove` take `--agent claude`, `codex`, or `gemini`
(repeatable), `--dry-run` to report what would change, and `--json`. With no
agent found, setup puts the skill alone in `~/.agents/skills`. A `btmux` skill
that btmux didn't install is left alone and reported as a conflict, and the
context hook isn't added; `--force` moves the skill to the trash and replaces
it.

Restart running agents to load new hooks. Codex also skips new and changed
hooks until you trust them: run `/hooks` in Codex and trust btmux's after
setup, and again after an upgrade changes them.

To paste the status hooks yourself instead, print them with
`btmux generate-claude-code-hooks`, `generate-codex-hooks`, or
`generate-gemini-cli-hooks`. The same guide is in the repository as
[`skills/btmux`](../../../../../skills/btmux/SKILL.md), which
`npx skills add btmux-dev/btmux` installs as a copy that doesn't follow
upgrades.

:::note
Codex runs hooks in a shared background daemon, which reports every turn to the
pane that started it. Run `codex --no-daemon` to report to the right pane.
:::

## States

| State       | Meaning                                                       |
| ----------- | ------------------------------------------------------------- |
| **working** | A turn is running                                             |
| **waiting** | The turn ended, but background work will resume it            |
| **blocked** | Waiting for you: a permission request or a question           |
| **done**    | The turn finished and you haven't looked yet                  |
| **idle**    | Waiting for a prompt                                          |
| **unknown** | An agent is running, but btmux can't tell what it is doing    |

The state appears as a badge on the pane, and the status bar marks windows
with a working agent. `done` becomes `idle` once you view the pane. Work an
agent leaves running between turns, such as Claude Code's background shells,
is shown beside the state. An agent at its prompt with such work is
**waiting**, since it starts again by itself once the work ends: `waiting on
2 shells, 1 monitor`. `btmux wait` and `wait_for_agent` keep waiting through
it.

## Agent view

`<prefix> + a` lists every agent on the server, grouped by state with the most
urgent first: **blocked**, then **done**, **working**, **waiting**, and
**idle**. Within a
group, the agent that has waited longest comes first.

Each row shows the pane title, agent message, time in state, session, window,
and Git branch. Click a row to go to its pane. The eye button opens a live
tile with GitHub status and reported cost. Each client remembers its tiles.

![The agent view with three agents: working, blocked, and done](../../../assets/screenshots/agent-view-dark.png)
![The agent view with three agents: working, blocked, and done](../../../assets/screenshots/agent-view-light.png)

| Key                  | Action                                          |
| -------------------- | ----------------------------------------------- |
| `Up` `Down`, `j` `k` | Move through the list                           |
| `Left` `Right`, `h` `l` | Move between tiles                           |
| `1`-`9`              | Select an agent                                 |
| `Enter`              | Go to that pane                                 |
| `Space`              | Show or hide its tile                           |
| `o`                  | Show only this agent's tile, and back to all    |
| `r`                  | Reset the agent's state                         |
| `x`                  | End the agent's session (asks first)            |
| `Tab`                | Switch to the [Usage](usage.md) tab             |
| `Esc`                | Close                                           |

## Notifications

btmux notifies you when an agent gets blocked, fails, or finishes. A turn
that ends with background work running notifies only if the agent doesn't
start again by itself: while it is **waiting**, the notification is held (up
to 10 minutes), and the turn it resumes notifies instead.

- **A dot** on the pane and its window, in every connected client. Viewing the
  pane clears it everywhere.
- **A toast**, unless you are already looking at that pane.
- **An OS notification** while the btmux window is hidden or unfocused. The
  first one offers to enable them, because browsers only grant the permission
  from a click. With several tabs or the desktop app connected, only the one
  you focused most recently notifies.

```toml
[notifications]
os = true
os-level = "attention"   # lowest level shown: info, success, attention, error
```

Your own scripts can notify too, with
[`btmux send-notification`](../automation/notifications.md).

## More

- [Track tokens, cost, and plan limits](usage.md)
- [Prompt agents from scripts](orchestrating.md)
- [Run agents in a sandbox](../features/sandbox.md)
- [Inspect or report agent state](detection.md)

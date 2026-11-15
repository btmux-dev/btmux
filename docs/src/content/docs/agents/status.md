---
title: "Agent status and notifications"
sidebar:
  order: 1
---

Agent integrations can report a live lifecycle state independently from
notifications:

```sh
curl -H "Authorization: Bearer $BTMUX_AUTH_TOKEN" \
  -X POST "${BTMUX_API_URL}/api/panes/${BTMUX_PANE_ID}/agent-status" \
  -H 'Content-Type: application/json' \
  -d '{"state":"working","agent":"codex","source":"hook"}'
```

The supported states are `unknown`, `idle`, `working`, `blocked`, and `done`.
`done` becomes `idle` when the pane is viewed. A tool completion or new turn
moves a blocked agent back to `working`; terminal input alone does not prove
that an approval was resolved. The existing `/notify` endpoint maps known hook
events to these transitions. A report's `turn_id` (Claude Code's `prompt_id`)
ties it to a turn: reports for a finished or replaced turn are ignored, and a
repeated start of the running turn, such as input queued into it, changes
nothing. Reports without turn identities can't be ordered this way. A
completion unblocks only the tool its permission named, by `tool_use_id` when
both send one, else by `tool_name` and `tool_input`. The completion's input
may add fields, as answering `AskUserQuestion` adds its answers, but must keep
every field of the permission's. Intermediate `TaskCompleted` events only notify;
they do not mark the whole turn done.

Use `DELETE` on the same endpoint when the reporter no longer knows which agent
is in the pane. Detected processes stay in the agent grid even when their
activity is `unknown`; explicit reports without a process need another state. Status is broadcast through the normal server-authoritative state snapshot,
so all connected browser tabs see the same value. Status resets when btmux
restarts. Explicit reports without a process identity expire after 12 hours
unless renewed.

## Hook notifications

Each pane's shell receives `BTMUX_PANE_ID`, `BTMUX_API_URL`, and `BTMUX_AUTH_TOKEN`. An agent harness
running in the pane can use them to display a colored dot or toast when it
stops, needs permission, fails, or finishes work—even when another pane or
session is active.

Attention and error notifications show a toast unless you are looking at that
pane, and an OS notification while the btmux window is hidden or unfocused. The
first one offers to enable OS notifications, because browsers only grant the
permission from a click. `[notifications]` in `config.toml` (or Settings →
Notifications) turns OS notifications off with `os = false` or changes the
lowest level shown with `os-level` (`info`, `success`, `attention`, or
`error`; default `attention`). When several browser tabs or the desktop app
are connected, only the one you focused most recently shows OS notifications. A permission `Notification` that repeats a pending
`PermissionRequest`, and an `idle_prompt` after the turn already finished, are
not shown again. A finished agent stays `done` until its pane is viewed in a
focused window.

The server keeps each pane's most severe unread notification in the state
snapshot, so every tab shows the same colored dot, and a reload keeps it.
Viewing the pane in any focused window, or `DELETE /notify`, clears it for all
tabs. Unread notifications are not saved across restarts.

btmux detects interactive Codex, Claude Code, and Gemini CLI processes under each
pane's shell, including known Node/Bun entry points. It checks process identity
roughly every second and uses the foreground job to select the agent whose
screen it reads. A confirmed process exit clears status even when Ctrl-C skips
the end hook. Suspended or background agents remain present with unknown activity.
When another agent takes the foreground, a suspended agent keeps its hook session
and resumes it after `fg`.
Press `prefix + a` to open the agent grid.

While a pane has a detected agent, the backend reconstructs its live terminal
screen from ordered PTY output and resize events, independently of browser
viewers. Tracking starts from the replay checkpoint and journal, so panes without
an agent pay no parsing cost. About every
300 ms, it evaluates bundled TOML manifests against the screen and OSC title and
progress signals. Rules recognize working indicators, approval dialogs, and idle
prompts. They scope matches to the current UI to avoid old transcript text.
Screen-only working/blocked transitions to idle or unknown settle for 700 ms to
avoid redraw flicker. Missing indicators do not prove completion: only
completion reports set `done`, and idle screens do not acknowledge it.

When hooks and screens disagree, the more reliable evidence wins:

1. `POST /agent-status` reports are reporter-controlled; screen detection never
   overrides them.
2. Hook reports outrank screens. A hook ignores screen evidence produced before
   it. An unmatched or `unknown` screen never overrides a hook, and a screen that
   agrees keeps the hook's source and message.
3. A matched screen rule that contradicts a hook for 1.5 s wins, which corrects
   missed hooks such as an interrupt without `Stop`. The next hook takes over again.
4. Process presence alone reports `unknown` activity.

Whatever set the state, the screen also fills `background` with the work an agent
leaves running between turns, such as Claude Code's background shells and
monitors (`"2 shells, 1 monitor"`), which can start a new turn when they finish.
The state stays as it is (an idle agent with a dev server running is still idle),
and status badges show it as `idle · 2 shells, 1 monitor`. It clears once the
screen has shown none for 700 ms.

Hooks may report a wrapper's child process; the process scan's own choice
identifies the agent and selects its manifest. Names reported by hooks take
precedence over process names. A newly detected agent ignores OSC titles that a
previous program left behind.

The generated hooks remain useful for session/turn identities, completion
messages, and notifications. Reinstall older hook snippets to receive those
reports. Agent status is runtime-only and resets when btmux restarts.

### Detection manifests and diagnostics

The bundled rules are adapted from Herdr revision
`5da0a01e1eedda054db0c81dd3a780000c40d9f0` (Claude's from
`bc8c28988f75c538d4ca058b702f5adbea30566e`); attribution and the upstream license
live in `third-party/herdr`. Codex is idle when its title has no spinner,
unless its working timer shows. Codex runs hooks in a shared app-server daemon,
whose environment names the pane that started it, so only `codex --no-daemon`
reports turns to the right pane. Unmatched screens produce unknown activity
rather than an inferred successful completion.

Local overrides replace each agent's bundled manifest:

```text
~/.config/btmux/agent-detection/claude.toml
~/.config/btmux/agent-detection/codex.toml
~/.config/btmux/agent-detection/gemini.toml
```

The directory follows `XDG_CONFIG_HOME`, like `config.toml`. Edits are checked
every five seconds. Invalid overrides log a warning and fall back to bundled
rules. Rules support priorities, scoped regions, `contains` (case-insensitive),
`regex`, per-line `line_regex`, nested `all`/`any`/`not`, visible evidence flags,
and `skip_state_update` for transcript viewers. An optional `[background]` table
(`region`, `regex`) names background work: the regex's first group, else its
whole match. Engine versions through 4 are supported. Manifests are local; btmux does not fetch remote rule updates.

For example, a custom Codex approval rule:

```toml
id = "codex"
version = "local.1"
min_engine_version = 3

[[rules]]
id = "custom_approval"
state = "blocked"
priority = 900
region = "after_last_prompt_marker"
visible_blocker = true
contains = ["enter to approve", "esc to cancel"]
```

Inspect status and detection evidence through the authenticated REST API:

```sh
curl -H "Authorization: Bearer $BTMUX_AUTH_TOKEN" \
  "${BTMUX_API_URL}/api/panes/${BTMUX_PANE_ID}/agent-status"
```

The response includes `status`, the `authority` that set it (`explicit`, `hook`,
`screen`, or `process`), tracked `process` identity, and the most recent
`detection`: matched rule, region, priority, manifest version/source, screen
revision, and visible evidence flags. It excludes terminal contents. A null rule
means no rule matched; a null detection means no screen evaluation is available.

Print ready-to-paste hook configuration with:

```sh
btmux generate-claude-code-hooks  # print JSON; merge it into ~/.claude/settings.json
btmux generate-codex-hooks        # print JSON; merge it into ~/.codex/hooks.json
btmux generate-gemini-cli-hooks   # print JSON; merge it into ~/.gemini/settings.json
```

Or install the hooks directly into the per-user config files while preserving
other settings and hook entries:

```sh
btmux install-claude-code-hooks
btmux install-codex-hooks
btmux install-gemini-cli-hooks
```

The Codex installer uses `$CODEX_HOME/hooks.json` when `CODEX_HOME` is set,
otherwise `~/.codex/hooks.json`.

The same snippets are available in [`extras/`](../../../../../extras/). Other command-hook
harnesses can POST their JSON hook payload to the authenticated endpoint:

```sh
curl -H "Authorization: Bearer $BTMUX_AUTH_TOKEN" \
  -H 'Content-Type: application/json' \
  -X POST --data-binary @- \
  "$BTMUX_API_URL/api/panes/$BTMUX_PANE_ID/notify"
```

Custom callers may add `title`, `body`, and `level`. A `title` or `body`
replaces only that part of the event's default text. Titles are capped at 200
characters and bodies at 1000.

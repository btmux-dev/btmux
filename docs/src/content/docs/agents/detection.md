---
title: "Status detection"
sidebar:
  order: 4
---

How btmux decides an agent's [state](index.md#states), and how to report or
adjust it yourself.

## Sources

btmux combines four kinds of evidence:

- **Processes.** It detects interactive Codex, Claude Code, and Gemini CLI
  processes under each pane's shell, including known Node/Bun entry points,
  about once a second. The foreground job selects the agent whose screen it
  reads. A confirmed process exit clears the status even when Ctrl+C skips the
  end hook. Suspended or background agents stay listed with unknown activity,
  and a suspended agent keeps its hook session and resumes it after `fg`.
- **Hooks.** The [installed hooks](index.md#setup) report session and turn
  starts, permission requests, tool completions, and the end of a turn.
- **The screen.** While a pane has a detected agent, btmux reconstructs its
  terminal screen and, about every 300 ms, matches bundled rules against it
  and against OSC title and progress signals. Rules recognize working
  indicators, approval dialogs, and idle prompts, scoped to the current UI so
  old transcript text doesn't match. Panes without an agent pay no parsing
  cost.
- **OSC 7501.** Programs can report their own state with
  [program status](https://www.superlogical.com/rex/docs/build/program-status)
  records; see [below](#osc-7501).

When they disagree, the more reliable evidence wins:

1. `POST /agent-status` reports are reporter-controlled; screen detection never
   overrides them.
2. OSC 7501 records outrank hooks and screens: they arrive in order with the
   program's output, while hooks report over HTTP and can trail it. Hooks from
   a new agent session still take over until the program reports again. With
   no records left, the other sources decide again.
3. Hook reports outrank screens. A hook ignores screen evidence produced before
   it. An unmatched or `unknown` screen never overrides a hook, and a screen that
   agrees keeps the hook's source and message.
4. A matched screen rule that contradicts a hook for 1.5 s wins, which corrects
   missed hooks such as an interrupt without `Stop`. The next hook takes over again.
5. Process presence alone reports `unknown` activity.

Some consequences:

- Missing indicators do not prove completion: only completion reports set
  `done`, and idle screens do not acknowledge it. A finished agent stays `done`
  until its pane is viewed in a focused window.
- Screen-only transitions from working or blocked to idle or unknown settle
  for 700 ms, to avoid redraw flicker.
- A permission `Notification` that repeats a pending `PermissionRequest`, and
  an `idle_prompt` after the turn already finished, are not shown again.
- Hooks may report a wrapper's child process; the process scan's own choice
  identifies the agent and selects its rules. Names reported by hooks take
  precedence over process names. A newly detected agent ignores OSC titles
  that a previous program left behind.
- Whatever set the state, the screen also fills `background` with the work an
  agent leaves running between turns (`"2 shells, 1 monitor"`). The state
  stays as it is: an idle agent with a dev server running is still idle. It
  clears once the screen has shown none for 700 ms.

Status is runtime-only and resets when btmux restarts. The generated hooks
remain useful for session and turn identities, completion messages, and
notifications. Reinstall older hook snippets to receive those reports.

## Inspecting a pane

```sh
curl -H "Authorization: Bearer $BTMUX_AUTH_TOKEN" \
  "${BTMUX_API_URL}/api/panes/${BTMUX_PANE_ID}/agent-status"
```

The response includes `status`, the `authority` that set it (`explicit`, `hook`,
`screen`, or `process`), the tracked `process` identity, and the most recent
`detection`: matched rule, region, priority, manifest version and source, screen
revision, and visible evidence flags. It excludes terminal contents. A null rule
means no rule matched; a null detection means no screen evaluation is available.

## Reporting status yourself

An integration can set the state directly:

```sh
curl -H "Authorization: Bearer $BTMUX_AUTH_TOKEN" \
  -X POST "${BTMUX_API_URL}/api/panes/${BTMUX_PANE_ID}/agent-status" \
  -H 'Content-Type: application/json' \
  -d '{"state":"working","agent":"codex","source":"hook"}'
```

The states are `unknown`, `idle`, `working`, `blocked`, and `done`. Use
`DELETE` on the same endpoint when the reporter no longer knows which agent is
in the pane. Reports without a process identity expire after 12 hours unless
renewed, and need a state other than `unknown` to appear in the agent view.

A report's `turn_id` (Claude Code's `prompt_id`) ties it to a turn: reports
for a finished or replaced turn are ignored, and a repeated start of the
running turn, such as input queued into it, changes nothing. Reports without
turn identities can't be ordered this way.

## OSC 7501

Programs can report their own state with OSC 7501
([program status](https://www.superlogical.com/rex/docs/build/program-status)),
and btmux answers its support queries. A pane with records counts as an agent,
named by the record's `app` unless a detected process or hook names it.

- The record most in need of attention sets the state: `blocked`, then
  `working`, `error`, `done`, and `idle`, the newest among equals.
- `error` is shown as `done` with an `Error:` message, and `kind` describes a
  blocked record without a message.
- As the spec says, `working`, `blocked`, and `idle` records end when their
  program's process group exits or a shell prompt starts (OSC 133 `A`), and RIS
  clears all of them.
- Records reported from the shell itself, with no program in the foreground,
  are ignored.
- An idle record does not acknowledge `done`, and a `done` or `error` result
  stays at the shell prompt until it is acknowledged or cleared.
- Records survive `btmux upgrade`.

## Hooks for other agents

Each pane's shell receives `BTMUX_PANE_ID`, `BTMUX_API_URL`, and
`BTMUX_AUTH_TOKEN`. Any harness with command hooks can POST its JSON hook
payload:

```sh
curl -H "Authorization: Bearer $BTMUX_AUTH_TOKEN" \
  -H 'Content-Type: application/json' \
  -X POST --data-binary @- \
  "$BTMUX_API_URL/api/panes/$BTMUX_PANE_ID/notify"
```

The existing `/notify` endpoint maps known hook events to state transitions:

- A tool completion or new turn moves a blocked agent back to `working`;
  terminal input alone does not prove that an approval was resolved.
- A completion unblocks only the tool its permission named, by `tool_use_id`
  when both send one, else by `tool_name` and `tool_input`. The completion's
  input may add fields, as answering `AskUserQuestion` adds its answers, but
  must keep every field of the permission's.
- Intermediate `TaskCompleted` events only notify; they do not mark the whole
  turn done.

Callers may add `title`, `body`, and `level`. A `title` or `body` replaces only
that part of the event's default text. Titles are capped at 200 characters and
bodies at 1000. `DELETE /notify` clears the pane's unread notification.

The snippets the installers write are also in
[`extras/`](../../../../../extras/).

## Screen rules

The bundled rules are adapted from Herdr revision
`5da0a01e1eedda054db0c81dd3a780000c40d9f0` (Claude's from
`bc8c28988f75c538d4ca058b702f5adbea30566e`); attribution and the upstream license
live in `third-party/herdr`. Codex is idle when its title has no spinner,
unless its working timer shows. Unmatched screens produce unknown activity
rather than an inferred successful completion.

A local file replaces an agent's bundled rules:

```text
~/.config/btmux/agent-detection/claude.toml
~/.config/btmux/agent-detection/codex.toml
~/.config/btmux/agent-detection/gemini.toml
```

The directory follows `XDG_CONFIG_HOME`, like `config.toml`. Edits are checked
every five seconds. Invalid files log a warning and fall back to the bundled
rules. btmux does not fetch rule updates.

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

Rules support priorities, scoped regions, `contains` (case-insensitive),
`regex`, per-line `line_regex`, nested `all`/`any`/`not`, visible evidence flags,
and `skip_state_update` for transcript viewers. An optional `[background]` table
(`region`, `regex`) names background work: the regex's first group, else its
whole match. Engine versions through 4 are supported.

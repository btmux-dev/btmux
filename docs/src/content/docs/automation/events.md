---
title: "Events"
sidebar:
  order: 7
---

`btmux events` reports changes on the server as they happen, so scripts can
react to them instead of polling: a pane closing, an agent finishing, a
session being renamed.

```sh
btmux events                      # one line per event, until interrupted
btmux events -s work              # only session "work"; exits once it closes
btmux events --json | jq -c 'select(.type == "agent_status_changed" and .status.state == "done")'
```

Without `--json`, each event is one line for reading:

```text
14:03:20  ready  btmux 0.0.215, 2 sessions
14:03:22  pane_created  work:1 (server) pane 3f2a…
14:05:40  agent_status_changed  work:0 (edit) pane 9c1d…  claude working -> done
```

Line breaks and control characters in names and messages are escaped (`\n`,
`\u{1b}`), so each event stays on one line and can't change the terminal.
With `--json`, each line is one JSON object, as the server sent it, written
as soon as the event happens.

Inside a pane, the command follows that pane's server; elsewhere, the running
server of `--profile`. Panes of [sandboxed sessions](../features/sandbox.md)
can't use it, since their credential reaches only their own panes.

## Events

Every event has `type` and `time` (Unix milliseconds). Events about a
session, window, or pane also have `session_id` and `session` (its name),
then `window_id`, `window`, and `window_index`, then `pane_id`.

| `type`                 | When                                              | Also has                    |
| ---------------------- | ------------------------------------------------- | --------------------------- |
| `ready`                | First, on every connection                        | `version`, `session_id`, `sessions` |
| `resync`               | The stream fell behind and events were lost       |                             |
| `session_created`      | A session was created                             |                             |
| `session_renamed`      | A session was renamed                             | `old_name`                  |
| `session_closed`       | A session was closed                              |                             |
| `window_created`       | A window was created                              |                             |
| `window_renamed`       | A window was renamed                              | `old_name`                  |
| `window_selected`      | A session's active window changed                 |                             |
| `window_closed`        | A window was closed                               |                             |
| `pane_created`         | A pane was created                                |                             |
| `pane_moved`           | A pane went to another window                     | `from_window_id`            |
| `pane_selected`        | A window's active pane changed                    |                             |
| `pane_zoomed`          | A pane was zoomed or unzoomed                     | `zoomed`                    |
| `pane_title_changed`   | A pane's title changed                            | `title`                     |
| `pane_cwd_changed`     | A pane's working directory changed                | `cwd`                       |
| `pane_closed`          | A pane was closed                                 |                             |
| `agent_status_changed` | An agent changed [state](../agents/index.md#states), or another agent took over | `previous`, `status` |
| `pane_notification`    | An agent hook or alert notified about a pane      | `event`, `level`, `title`, `body` |
| `notification`         | [`btmux send-notification`](notifications.md)     | `level`, `title`, `body`    |
| `widget_opened`, `widget_closed` | A [widget](widgets.md) opened or closed over a pane, or over its popup | `widget_id`, `kind`, `popup_id` |
| `popup_opened`, `popup_closed`   | A [popup](popups.md) opened or closed over a pane   | `popup_id`, `title` |
| `config_changed`       | The config was reloaded or changed for the session | `error`                    |
| `toast`                | A message for every client, such as an upgrade starting | `level`, `message`    |

A change that creates or closes several things reports each one: closing a
session reports its panes, then its windows, then the session. Events about a
closed pane, window, or widget name it as it was.

## Keeping track

`ready`'s `sessions` holds the sessions, windows, and panes as they were when
the stream started, in the shape `GET /api/sessions/<id>` returns. Each event
after it is a change to that state, so a script can start from `sessions` and
apply events to stay current.

A stream that falls far enough behind, such as one whose reader stopped
reading, skips ahead and sends `resync`. The events after it report every
session, window, and pane change since the last event before it, so tracked
state is current again. What it can't report is lost: a change undone in
between (an agent that went to `working` and back), notifications, toasts,
and widgets and popups opening or closing.

With `-s` (`--session`, an ID or a name), only that session's events are
reported, and the command exits once the session closes. Notifications not
about a pane, config changes, and toasts are left out.

## Reconnecting

If the connection drops, for example on `btmux upgrade` or a restart, the
command says so on stderr and reconnects every second until the server is
back. The new connection starts with `ready` again, whose `sessions` replaces
what you tracked: changes while it was disconnected aren't reported. It exits
1 if the server isn't running when it starts, or refuses it, and 0 once
nothing reads its output, such as after `| head -n 1`.

## Over HTTP

`GET /api/events` streams the same JSON lines, and takes `session` as a query
parameter. Empty lines every 30 seconds keep the connection open.

```sh
curl -sN -H "Authorization: Bearer $BTMUX_AUTH_TOKEN" "$BTMUX_API_URL/api/events"
```

## Internals

Browser tabs get whole snapshots of the session tree over `/ws/control`. One
translator per server compares each snapshot with the previous one and turns
it, and the other messages, into events once; each stream then keeps the
events for its session. If the translator falls behind the control messages,
or a stream behind the translator (256 messages), it compares the last state
it saw with the current one instead and sends `resync` first. UI messages for
tabs, such as opening the file browser, and agent telemetry aren't events.

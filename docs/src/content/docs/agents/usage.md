---
title: "Usage and telemetry"
sidebar:
  order: 2
---

Agent view's **Usage** tab graphs running totals of tokens, completed requests,
tool calls, and reported costs over the selected range, for all panes or one
selected pane, broken down by model. That includes helper models an agent picks
itself. Usage reported without a model, such as Claude's tool calls, is listed as
Unattributed. It keeps 24 hours of
per-minute history in memory, including panes that have since closed. `btmux
upgrade` keeps it; restarting the server clears it. The 8- and 24-hour ranges
group usage into 5-minute points.
Telemetry does not change agent lifecycle state or replace notification hooks.

Launch agents through btmux to enable collection for the current pane:

```sh
btmux claude --resume
btmux codex
btmux codex exec "Explain this project"
```

Every argument after `claude` or `codex` goes to that agent, including `--help`,
`--version`, `--profile`, and `--`. Both wrappers use the executable on `PATH`
and inherit the working directory and terminal streams. On Unix the agent
replaces the wrapper, preserving its PID, signals, and exit status; Windows
runs the agent, including an npm `.cmd` shim, and returns its exit code. Missing
executables exit with 127.

Inside a pane, `btmux claude` sets the log and metric exporter environment for
this invocation. Exports use HTTP/protobuf with 10-second metric and 5-second
log intervals unless you already set custom intervals. `btmux codex` prepends
an `otel.exporter` config override; its authorization value comes from a child
environment variable, so the credential never enters the command line. Codex
arguments that follow can override the injected config; managed agent settings
keep their normal precedence. Neither wrapper edits agent config files.

Outside a btmux pane, these commands run the raw agent without adding telemetry
settings. Inside a pane with missing or invalid telemetry configuration, they
print an error and exit with 1. Older panes derive their endpoint from
`BTMUX_API_URL` and `BTMUX_PANE_ID` when `BTMUX_OTEL_ENDPOINT` is absent.

The manual setup below is also available for launching agents directly.

The OTLP/HTTP receiver runs on the existing authenticated server. Each new pane
gets `BTMUX_OTEL_ENDPOINT`, a base URL that associates exports with that pane:

```text
$BTMUX_API_URL/api/panes/$BTMUX_PANE_ID/telemetry
```

It accepts `POST .../v1/logs` and `POST .../v1/metrics`, with
`application/x-protobuf` or `application/json`, and optional gzip compression.
Requests require `Authorization: Bearer $BTMUX_AUTH_TOKEN`, just like other
APIs. `GET /api/telemetry` returns the current numeric summaries, including
closed panes. Control sockets get the full history when they connect, then only
the minutes that change.

Only numeric usage, the agent name, and the model names are retained. Prompts, tool
arguments/results, and raw log records are discarded. Batches are limited to
8 MiB after decompression, and stored pane and deduplication histories
are bounded. Subscription quotas are not collected by this receiver.

`GET /api/agent-limits` returns those quotas for the Claude Code and Codex accounts
logged in on the server: each limit's label, percent used, reset time, and
severity. btmux asks each CLI, without sending a prompt, and shares a read for
50 seconds. An agent without a subscription login isn't listed.

:::caution
Both CLI interfaces are undocumented or experimental, so a CLI update can make
an agent disappear from this list.
:::

## Claude Code

Start Claude Code with both metrics and logs enabled:

```sh
CLAUDE_CODE_ENABLE_TELEMETRY=1 \
OTEL_METRICS_EXPORTER=otlp \
OTEL_LOGS_EXPORTER=otlp \
OTEL_EXPORTER_OTLP_METRICS_TEMPORALITY_PREFERENCE=delta \
OTEL_EXPORTER_OTLP_PROTOCOL=http/protobuf \
OTEL_EXPORTER_OTLP_ENDPOINT="$BTMUX_OTEL_ENDPOINT" \
OTEL_EXPORTER_OTLP_HEADERS="Authorization=Bearer $BTMUX_AUTH_TOKEN" \
OTEL_METRIC_EXPORT_INTERVAL=10000 \
OTEL_LOGS_EXPORT_INTERVAL=5000 \
claude
```

Tokens and costs come from `claude_code.token.usage` and
`claude_code.cost.usage` metrics, which must use delta temporality. Cumulative
points are ignored with a warning in the server log, so requests and tool calls
would still appear with no tokens or cost. Request and tool activity comes from `api_request` and `tool_result`
logs. Token counts in those logs are ignored so enabling both exports does not
count usage twice. Cached input is included in input, and is also shown as a
separate subset. Reported cost is the agent's estimate, not a subscription bill.

## Codex CLI

Configure the log exporter for this invocation:

```sh
OTEL_EXPORTER_OTLP_LOGS_HEADERS="Authorization=Bearer%20$BTMUX_AUTH_TOKEN" \
codex \
  -c "otel.exporter={otlp-http={endpoint=\"${BTMUX_OTEL_ENDPOINT}/v1/logs\",protocol=\"binary\"}}"
```

The header stays in the environment because Codex sends a `${VAR}` header from
`-c` literally, which the receiver rejects with a 401. These flags leave the
user's config file unchanged. Token counts and completed requests come from
`response.completed` log events; tool calls come from `codex.tool_result`.
Codex token metrics are ignored to avoid counting both log and metric exports.
Cached tokens are part of input, and reasoning tokens are part of output.
Costs remain unavailable unless a supported source reports them; btmux does
not infer prices.

For panes created before this feature was installed, set the base URL first:

```sh
export BTMUX_OTEL_ENDPOINT="$BTMUX_API_URL/api/panes/$BTMUX_PANE_ID/telemetry"
```

See the [Claude Code monitoring guide](https://code.claude.com/docs/en/monitoring-usage)
and [Codex telemetry configuration](https://learn.chatgpt.com/docs/config-file/config-advanced#observability-and-telemetry)
for exporter settings. This receiver supports OTLP/HTTP; select an HTTP exporter
rather than OTLP/gRPC.

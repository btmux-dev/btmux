---
title: "Usage and plan limits"
sidebar:
  order: 2
---

Open **Usage** in the agent view (`<prefix> + a`, then Tab) to see plan limits
and per-model totals for tokens, completed requests, tool calls, and reported cost.

## Collecting usage

Launch agents through btmux to track usage by pane.

```sh
btmux claude --resume
btmux codex
btmux codex exec "Explain this project"
```

The wrappers forward arguments and set telemetry options for that run.
Outside a btmux pane they run the agent unchanged.

## Reading the charts

- Charts show running totals over the last 15 minutes, hour, 8 hours, or 24
  hours, for all panes or one.
- Usage is broken down by model, including helper models an agent picks
  itself. Usage reported without a model, such as Claude's tool calls, is
  listed as Unattributed.
- Cached input is included in input and also shown on its own. For Codex,
  reasoning tokens are part of output.
- Cost is the agent's own estimate, not a subscription bill. btmux does not
  infer prices, so Codex shows none.

History covers 24 hours, including panes that have since closed. It is kept in
memory: `btmux upgrade` keeps it, and restarting the server clears it.

## Plan limits

For Claude Code, Codex, and Antigravity accounts logged in on the server, Usage
shows the percentage used and reset time for each subscription limit, including
use outside btmux. Antigravity's limits are shared by groups of models, so each
is named by its window and group. Agents using an API key, or not logged in,
aren't listed.

btmux reads limits through the CLI without sending a prompt or reading
credentials. `GET /api/agent-limits` returns the same data.

:::caution
These CLI interfaces are undocumented or experimental, so a CLI update can make
an agent disappear from this list.
:::

## What is kept

Only numbers, the agent name, and model names. Prompts, tool arguments and
results, and raw log records are discarded. `GET /api/telemetry` returns the
stored summaries.

## Manual setup

To launch an agent yourself, point its OpenTelemetry exporter at the pane's
receiver. Each pane has `BTMUX_OTEL_ENDPOINT`:

```text
$BTMUX_API_URL/api/panes/$BTMUX_PANE_ID/telemetry
```

It accepts OTLP/HTTP (not gRPC): `POST .../v1/logs` and `POST .../v1/metrics`,
as `application/x-protobuf` or `application/json`, optionally gzipped, up to
8 MiB after decompression. Requests need `Authorization: Bearer
$BTMUX_AUTH_TOKEN`, like other APIs.

### Claude Code

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

Tokens and costs come from the `claude_code.token.usage` and
`claude_code.cost.usage` metrics, which must use delta temporality. Cumulative
points are ignored with a warning in the server log, so requests and tool calls
would still appear with no tokens or cost. Requests and tool calls come from
`api_request` and `tool_result` logs; token counts in those logs are ignored,
so nothing counts twice. See the
[Claude Code monitoring guide](https://code.claude.com/docs/en/monitoring-usage).

### Codex

```sh
OTEL_EXPORTER_OTLP_LOGS_HEADERS="Authorization=Bearer%20$BTMUX_AUTH_TOKEN" \
codex \
  -c "otel.exporter={otlp-http={endpoint=\"${BTMUX_OTEL_ENDPOINT}/v1/logs\",protocol=\"binary\"}}"
```

The header stays in the environment because Codex sends a `${VAR}` header from
`-c` literally, which the receiver rejects with a 401. Token counts and
completed requests come from `response.completed` log events; tool calls come
from `codex.tool_result`. Codex's token metrics are ignored. See
[Codex telemetry configuration](https://learn.chatgpt.com/docs/config-file/config-advanced#observability-and-telemetry).

### How the wrappers differ

- They use the agent executable on `PATH`. On Unix the agent replaces the
  wrapper, keeping its PID, signals, and exit status; Windows runs the agent,
  including an npm `.cmd` shim, and returns its exit code. A missing
  executable exits with 127.
- Export intervals are 10 seconds for metrics and 5 for logs, unless you set
  your own.
- Codex's credential comes from a child environment variable, so it never
  enters the command line. Codex arguments that follow can override the
  injected config.
- Inside a pane with missing or invalid telemetry settings, they print an
  error and exit with 1.

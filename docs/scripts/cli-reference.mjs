// Writes the "All commands" page from the btmux binary's --help output.
// Usage: node docs/scripts/cli-reference.mjs <path to btmux>
import { execFileSync } from "node:child_process";
import { writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const bin = process.argv[2];
if (!bin) throw new Error("usage: cli-reference.mjs <btmux binary>");
const out = join(
  dirname(fileURLToPath(import.meta.url)),
  "../src/content/docs/configuration/commands.md",
);
const help = (...args) =>
  execFileSync(bin, [...args, "--help"], { encoding: "utf8" }).trimEnd();

// Sections in reading order; a command not listed here fails the run.
const groups = {
  "Service and upgrades": ["install", "uninstall", "restart", "upgrade", "connect", "version"],
  "Agents": ["claude", "codex", "agents", "wait", "prompt"],
  "From a pane": ["open-editor", "popup", "pick", "fzf", "confirm", "input", "send-notification"],
  "Sessions, windows, and panes": [
    "list-sessions", "list-windows", "list-panes", "new-session", "new-window",
    "split-window", "kill-pane", "kill-window", "kill-session", "select-pane",
    "select-window", "rename-window", "rename-session", "send-keys",
    "capture-pane", "new-worktree", "remove-worktree",
  ],
  "Setup": [
    "completions", "generate-config", "install-claude-code-hooks",
    "install-codex-hooks", "install-gemini-cli-hooks",
    "generate-claude-code-hooks", "generate-codex-hooks",
    "generate-gemini-cli-hooks", "install-fish-integration",
    "generate-fish-theme",
  ],
};

const top = help();
const listed = top
  .slice(top.indexOf("Commands:"), top.indexOf("Options:"))
  .split("\n")
  .map((line) => line.match(/^  ([a-z0-9-]+) /)?.[1])
  .filter((name) => name && name !== "help");
const grouped = Object.values(groups).flat();
const missing = listed.filter((name) => !grouped.includes(name));
const gone = grouped.filter((name) => !listed.includes(name));
if (missing.length || gone.length)
  throw new Error(`update groups: missing ${missing}; gone ${gone}`);

const fence = (text) => "```text\n" + text + "\n```\n";
const section = (name) => {
  const text = help(name);
  const usage = text.indexOf("Usage:");
  const about = text.slice(0, usage).trim().replace(/\s*\n\s*/g, " ");
  // Without the help flag every command has, and an Options heading it leaves empty.
  const body = text
    .slice(usage)
    .replace(/^ +(-h, )?--help +Print help.*\n?/gm, "")
    .replace(/\n+Options:\s*$/, "")
    .trimEnd();
  return `### \`btmux ${name}\`\n\n${about}\n\n${fence(body)}`;
};

const page = `---
title: "All commands"
sidebar:
  order: 4
---

Every \`btmux\` command and flag, as \`--help\` prints them. Run without a
command, \`btmux\` starts the server.

Inside a pane, commands talk to that pane's server. Elsewhere they find the
running server of \`--profile\`, or the default profile.

## Server

${fence(top.slice(top.indexOf("Usage:"), top.indexOf("Commands:")).trim() + "\n\n" + top.slice(top.indexOf("Options:")))}
## Environment

Set in every pane's shell:

| Variable              | Value                                             |
| --------------------- | ------------------------------------------------- |
| \`BTMUX_PANE_ID\`       | The pane's ID, the default target of commands     |
| \`BTMUX_API_URL\`       | The server's address, for the [REST API](../automation/rest-api.md) |
| \`BTMUX_AUTH_TOKEN\`    | The [access token](access.md)                     |
| \`BTMUX_OTEL_ENDPOINT\` | Where agents send [usage](../agents/usage.md)     |
| \`BTMUX_BIN\`           | The server's own binary                           |
| \`TERM_PROGRAM\`        | \`btmux\`                                           |

Read by the server: \`BTMUX_AUTH_TOKEN\` sets the access token instead of the
token file, and \`BTMUX_CONSOLE_LOG\` and \`BTMUX_FILE_LOG\` override the
\`[log]\` levels.

${Object.entries(groups)
  .map(([title, names]) => `## ${title}\n\n${names.map(section).join("\n")}`)
  .join("\n")}`;

writeFileSync(out, page);
console.log(`wrote ${out}`);

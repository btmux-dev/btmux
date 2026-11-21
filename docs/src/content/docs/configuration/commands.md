---
title: "All commands"
sidebar:
  order: 4
---

Every `btmux` command and flag, as `--help` prints them. Run without a
command, `btmux` starts the server.

Inside a pane, commands talk to that pane's server. Elsewhere they find the
running server of `--profile`, or the default profile.

## Server

```text
Usage: btmux [OPTIONS] [COMMAND]

Options:
      --host <HOST>              [default: 127.0.0.1]
      --public-url <PUBLIC_URL>  Additional allowed HTTP(S) origins, e.g. a reverse proxy or Vite dev URL
  -p, --port <PORT>              [default: 8004]
      --profile <PROFILE>        Use an isolated, named session state profile. Without a profile, sessions are stored in the default state.json
      --shell <SHELL>            Shell to spawn. Overrides `shell` in config.toml when passed explicitly
      --no-browser               Do not open a browser tab on startup
      --tailscale                Serve over HTTPS on the tailnet (`tailscale serve`), at this port of the machine's tailnet name
  -h, --help                     Print help
```

## Environment

Set in every pane's shell:

| Variable              | Value                                             |
| --------------------- | ------------------------------------------------- |
| `BTMUX_PANE_ID`       | The pane's ID, the default target of commands     |
| `BTMUX_API_URL`       | The server's address, for the [REST API](../automation/rest-api.md) |
| `BTMUX_AUTH_TOKEN`    | The [access token](access.md)                     |
| `BTMUX_OTEL_ENDPOINT` | Where agents send [usage](../agents/usage.md)     |
| `BTMUX_BIN`           | The server's own binary                           |
| `TERM_PROGRAM`        | `btmux`                                           |

Read by the server: `BTMUX_AUTH_TOKEN` sets the access token instead of the
token file, and `BTMUX_CONSOLE_LOG` and `BTMUX_FILE_LOG` override the
`[log]` levels.

## Service and upgrades

### `btmux install`

Install btmux as a per-user background service so it starts at login and restarts on crash. The current `--host`/`--port`/`--profile`/`--shell`/ `--public-url` and the installing shell's PATH are baked into the generated unit. Uses launchd on macOS and systemd on Linux

```text
Usage: btmux install [OPTIONS]

Options:
      --print  Print the generated service unit to stdout instead of installing it
```

### `btmux uninstall`

Uninstall the background service created by `install` (stop it and remove its service unit)

```text
Usage: btmux uninstall
```

### `btmux restart`

Restart the background service (e.g. after updating the btmux binary)

```text
Usage: btmux restart
```

### `btmux upgrade`

Move the running `--profile` server to this binary in place, keeping its shells and scrollback

```text
Usage: btmux upgrade
```

### `btmux connect`

Connect this server to btmux connect with an invite code, to reach it from anywhere at its own `*.btmux.net` address

```text
Usage: btmux connect [OPTIONS] [INVITE]

Arguments:
  [INVITE]  An invite code (`btmi_…`); asked for if not given

Options:
      --accept-acme-terms  Agree to Let's Encrypt's Subscriber Agreement (https://letsencrypt.org/repository/), which issues this machine's certificate, without being asked
```

### `btmux version`

Print the btmux version and exit

```text
Usage: btmux version
```

## Agents

### `btmux claude`



```text
Usage: claude [options] [command] [prompt]

Claude Code - starts an interactive session by default, use -p/--print for
non-interactive output

Arguments:
  prompt                                Your prompt

Options:
  --add-dir <directories...>            Additional directories to allow tool
                                        access to
  --agent <agent>                       Agent for the current session. Overrides
                                        the 'agent' setting.
  --agents <json-or-file>               JSON object defining custom agents, or
                                        with --print the path to a file that
                                        holds one (e.g. '{"reviewer":
                                        {"description": "Reviews code",
                                        "prompt": "You are a code reviewer"}}')
  --allow-dangerously-skip-permissions  Enable bypassing all permission checks
                                        as an option, without it being enabled
                                        by default. Recommended only for
                                        sandboxes with no internet access.
  --allowedTools, --allowed-tools <tools...>
      Comma or space-separated list of tool names to allow (e.g. "Bash(git *)
      Edit")
  --append-system-prompt <prompt>       Append a system prompt to the default
                                        system prompt
  --autocompact <auto|tokens>           Auto-compact window size (auto, or
                                        100k–1M tokens)
  --ax-screen-reader                    Render screen-reader friendly output
                                        (flat text, no decorative borders or
                                        animations).
  --bg, --background                    Start the session in the background and
                                        return immediately. Prints the id that
                                        `claude attach`, `logs`, `stop` and `rm`
                                        take; `claude agents` lists them. With
                                        --resume <session-id>, continues that
                                        session in the background under the same
                                        ID, or starts a copy and says so when
                                        the session is already running
  --bare                                Minimal mode: skip hooks (those defined
                                        in settings and by installed plugins;
                                        features built into Claude Code are
                                        unaffected), LSP, plugin sync,
                                        attribution, auto-memory, background
                                        prefetches, keychain reads, and
                                        CLAUDE.md auto-discovery. Sets
                                        CLAUDE_CODE_SIMPLE=1. Anthropic auth is
                                        strictly ANTHROPIC_API_KEY or
                                        apiKeyHelper via --settings (OAuth and
                                        keychain are never read). 3P providers
                                        (Bedrock/Vertex/Foundry) use their own
                                        credentials. Skills still resolve via
                                        /skill-name. Explicitly provide context
                                        via: --system-prompt[-file],
                                        --append-system-prompt[-file], --add-dir
                                        (CLAUDE.md dirs), --mcp-config,
                                        --settings, --agents, --plugin-dir.
  --betas <betas...>                    Beta headers to include in API requests
                                        (API key users only)
  --brief                               Enable SendUserMessage tool for
                                        agent-to-user communication
  --chrome                              Enable Claude in Chrome integration
  --cloud [description|session_id|url]  Create a cloud session with the given
                                        description, or attach to an existing
                                        one by session ID or claude.ai/code URL
  -c, --continue                        Continue the most recent conversation in
                                        the current directory
  --dangerously-skip-permissions        Bypass all permission checks.
                                        Recommended only for sandboxes with no
                                        internet access.
  -d, --debug [filter]                  Enable debug mode with optional category
                                        filtering (e.g., --debug=hooks,mcp or
                                        --debug='!hooks,!mcp')
  --debug-file <path>                   Write debug logs to a specific file path
                                        (implicitly enables debug mode)
  --desktop                             Open in the Claude Desktop app instead
                                        of the terminal (with --continue or
                                        --resume <id> to pick the session)
  --disable-slash-commands              Disable all skills and commands
  --disallowedTools, --disallowed-tools <tools...>
      Comma or space-separated list of tool names to deny (e.g. "Bash(git *)
      Edit")
  --effort <level>                      Effort level for the current session
                                        (low, medium, high, xhigh, max)
  --environment <environment_id>        Create a new cloud session that runs on
                                        the given self-hosted environment
                                        (ccpool_...).
  --exclude-dynamic-system-prompt-sections
      Move per-machine sections (cwd, env info, memory paths, git status) from
      the system prompt into the first user message. Improves cross-user
      prompt-cache reuse. Only applies with the default system prompt (ignored
      with --system-prompt). (default: false)
  --fallback-model <model>              Enable automatic fallback to specified
                                        model(s) when the default model is
                                        overloaded or not available. Accepts a
                                        comma-separated list to try each in
                                        order. Re-tries the primary at the start
                                        of each user turn.
  --file <specs...>                     File resources to download at startup.
                                        Format: file_id:relative_path (e.g.,
                                        --file file_abc:doc.txt
                                        file_def:img.png)
  --fork-session                        When resuming, create a new session ID
                                        instead of reusing the original (use
                                        with --resume or --continue)
  --forward-subagent-text               Forward subagent text and thinking
                                        blocks as assistant/user messages with
                                        parent_tool_use_id set (only works with
                                        --print and --output-format=stream-json)
  --from-pr [value]                     Resume a session linked to a PR by PR
                                        number/URL, or open interactive picker
                                        with optional search term
  -h, --help                            Display help for command
  --ide                                 Automatically connect to IDE on startup
                                        if exactly one valid IDE is available
  --include-hook-events                 Include all hook lifecycle events in the
                                        output stream (only works with
                                        --output-format=stream-json)
  --include-partial-messages            Include partial message chunks as they
                                        arrive (only works with --print and
                                        --output-format=stream-json)
  --input-format <format>               Input format (only works with --print):
                                        "text" (default), or "stream-json"
                                        (realtime streaming input) (choices:
                                        "text", "stream-json")
  --json-schema <schema>                JSON Schema for structured output
                                        validation (only works with --print).
                                        Example:
                                        {"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}
  --max-budget-usd <amount>             Maximum dollar amount to spend on API
                                        calls (only works with --print)
  --mcp-config <configs...>             Load MCP servers from JSON files or
                                        strings (space-separated)
  --model <model>                       Model for the current session. Provide
                                        an alias for the latest model (e.g.
                                        'fable', 'opus', or 'sonnet') or a
                                        model's full name.
  -n, --name <name>                     Set a display name for this session
                                        (shown in the prompt box, /resume
                                        picker, and terminal title)
  --no-chrome                           Disable Claude in Chrome integration
  --no-session-persistence              Disable session persistence - sessions
                                        will not be saved to disk and cannot be
                                        resumed (only works with --print)
  --output-format <format>              Output format (only works with --print):
                                        "text" (default), "json" (single
                                        result), or "stream-json" (realtime
                                        streaming) (choices: "text", "json",
                                        "stream-json")
  --permission-mode <mode>              Permission mode to use for the session
                                        (choices: "acceptEdits", "auto",
                                        "bypassPermissions", "manual",
                                        "dontAsk", "plan")
  --permission-prompts <target>         Who answers permission prompts with
                                        --print: "host" (the SDK host or
                                        --permission-prompt-tool) or "none"
                                        (nobody: anything that would prompt is
                                        denied automatically; the permission
                                        mode still decides everything else)
                                        (choices: "host", "none", default:
                                        "host")
  --plugin-dir <path>                   Load a plugin from a directory or .zip
                                        for this session only; a folder of
                                        plugins loads each child (repeatable:
                                        --plugin-dir A --plugin-dir B.zip)
                                        (default: [])
  --plugin-url <url>                    Fetch a plugin .zip from a URL for this
                                        session only (repeatable: --plugin-url A
                                        --plugin-url B) (default: [])
  -p, --print                           Print response and exit (useful for
                                        pipes). Note: The workspace trust dialog
                                        is skipped when Claude is run in
                                        non-interactive mode (via -p, or when
                                        stdout is not a TTY, e.g. piped or
                                        redirected output). Only use this in
                                        directories you trust. Settings files
                                        that fail validation are silently
                                        ignored in this mode (no error dialog is
                                        shown).
  --prompt-suggestions [value]          Enable prompt suggestions. In print/SDK
                                        mode, emits a prompt_suggestion message
                                        after each turn with a predicted next
                                        user prompt (choices: "true", "false",
                                        "1", "0", "yes", "no", "on", "off",
                                        preset: "true")
  --remote-control [name]               Start an interactive session with Remote
                                        Control enabled (optionally named)
  --remote-control-session-name-prefix <prefix>
      Prefix for auto-generated Remote Control session names (default: hostname)
  --replay-user-messages                Re-emit user messages from stdin back on
                                        stdout for acknowledgment (only works
                                        with --input-format=stream-json and
                                        --output-format=stream-json)
  --restricted                          Restricted mode: removes the built-in
                                        tools that run commands or code (Bash,
                                        PowerShell and the other code-running
                                        tools) and WebFetch unless --tools names
                                        them, and ignores user, project and
                                        local settings files (managed settings
                                        and --settings still apply; add
                                        --strict-mcp-config to skip MCP servers
                                        too). Also confines the file tools to
                                        the working directories (--add-dir
                                        included), refuses bypassPermissions,
                                        and lets only a person or the configured
                                        permission handler approve writes to
                                        settings, git and tool-configuration
                                        files.
  -r, --resume [value]                  Resume a conversation by session ID or
                                        name, or open interactive picker with
                                        optional search term
  --safe-mode                           Start with all customizations
                                        (CLAUDE.md, skills, installed plugins,
                                        hooks, MCP servers, custom commands and
                                        agents, output styles, workflows, custom
                                        themes, keybindings, and more) disabled
                                        — useful for troubleshooting a broken
                                        configuration. Admin-managed (policy)
                                        settings still apply. Auth, model
                                        selection, built-in tools and plugins,
                                        and permissions work normally. Sets
                                        CLAUDE_CODE_SAFE_MODE=1.
  --session-id <uuid>                   Use a specific session ID for the
                                        conversation (must be a valid UUID)
  --setting-sources <sources>           Comma-separated list of setting sources
                                        to load (user, project, local).
  --settings <file-or-json>             Path to a settings JSON file or a JSON
                                        string to load additional settings from
  --strict-mcp-config                   Only use MCP servers from --mcp-config,
                                        ignoring all other MCP configurations
  --system-prompt <prompt>              System prompt to use for the session
  --system-prompt-snapshot <on|off>     Record the system prompt once per
                                        conversation and reuse it verbatim on
                                        every request and resume. on (the
                                        default, except under --bare outside
                                        cloud and background sessions): the
                                        prompt is rendered on the conversation's
                                        first request — a --system-prompt or
                                        --append-system-prompt included — sent,
                                        and recorded; every later request and
                                        resume sends the record as-is, even when
                                        a later launch passes different text,
                                        until the conversation is compacted.
                                        off: never record; the prompt is
                                        rendered fresh every request (for
                                        iterating on prompt text). (choices:
                                        "on", "off")
  --teleport [session]                  Resume a teleport session, optionally
                                        specify session ID
  --tmux                                Create a tmux session for the worktree
                                        (requires --worktree). Uses iTerm2
                                        native panes when available; use
                                        --tmux=classic for traditional tmux.
  --tools <tools...>                    Specify the list of available tools from
                                        the built-in set. Use "" to disable all
                                        tools, "default" to use the default set,
                                        or specify tool names (e.g.
                                        "Bash,Edit,Read").
  --verbose                             Override verbose mode setting from
                                        config
  -v, --version                         Output the version number
  -w, --worktree [name]                 Create a new git worktree for this
                                        session (optionally specify a name)

Commands:
  agents [options]                      Manage background agents
  attach <id|name>                      Open a background session in this
                                        terminal. <id> is the short id that
                                        `claude --bg` prints and `claude agents`
                                        lists. Part of the session name works
                                        too
  auth                                  Manage authentication
  auto-mode                             Inspect or reset auto mode configuration
  doctor                                Check the health of your Claude Code
                                        installation. Reads settings files in
                                        the current directory without a trust
                                        prompt. For a full checkup that can also
                                        fix issues, run /doctor in a session.
  gateway [options]                     Run the enterprise auth/telemetry
                                        gateway
  import [options] [source]             Import config from another AI coding
                                        agent into Claude Code
  install [options] [target]            Install Claude Code native build. Use
                                        [target] to specify version (stable,
                                        latest, or specific version)
  logs <id|name>                        Print a background session's recent
                                        terminal output
  mcp                                   Configure and manage MCP servers
  plugin|plugins                        Manage Claude Code plugins
  purge [options] [path]                Delete a project's Claude Code data
                                        (transcripts, tasks, file history, and
                                        its entry in ~/.claude.json) from this
                                        computer only
  respawn [options] [id]                Restart a background session, or all of
                                        them with --all, so it runs the current
                                        Claude Code version
  rm <id>                               If it's safe, remove a background
                                        session from the list and delete its
                                        worktree. Its conversation is kept:
                                        `claude --resume` lists it. Works on
                                        sessions that have already exited
  setup-token                           Set up a long-lived authentication token
                                        (requires Claude subscription)
  stop|kill <id>                        Stop a background session. Its
                                        conversation is kept: `claude attach
                                        <id>` opens it again, `claude --resume`
                                        works once it is stopped
  ultrareview [options] [target]        Run a cloud-hosted multi-agent code
                                        review of the current branch (or a PR
                                        number / base branch) and print the
                                        findings
  update|upgrade                        Check for updates and install if
                                        available; package-manager installs
                                        update through the package manager
```

### `btmux codex`

Codex CLI If no subcommand is specified, options will be forwarded to the interactive CLI.

```text
Usage: codex [OPTIONS] [PROMPT]
       codex [OPTIONS] <COMMAND> [ARGS]

Commands:
  agents            Open the agent command center to browse and switch sessions
  exec              Run Codex non-interactively [aliases: e]
  review            Run a code review non-interactively
  login             Manage login
  logout            Remove stored authentication credentials
  mcp               Manage external MCP servers for Codex
  plugin            Manage Codex plugins
  app-server        [experimental] Run the app server or related tooling
  remote-control    [experimental] Manage the app-server daemon with remote control enabled
  app               Launch the Desktop app (opens the app installer if missing)
  completion        Generate shell completion scripts
  update            Update Codex to the latest version
  doctor            Diagnose local Codex installation, config, auth, and runtime health
  sandbox           Run commands within a Codex-provided sandbox
  debug             Debugging tools
  apply             Apply the latest diff produced by Codex agent as a `git apply` to your local
                    working tree [aliases: a]
  resume            Resume a previous interactive session (picker by default; use --last to continue
                    the most recent)
  queue             Queue a message for an existing session
  archive           Archive a saved session by id or session name
  delete            Permanently delete a saved session by id or session name
  migrate-rollouts  Inspect or migrate legacy local sessions to paginated thread history
  migrate-nfs-db    [experimental] Migrate configured SQLite databases to DELETE mode in place
  unarchive         Unarchive a saved session by id or session name
  fork              Fork a previous interactive session (picker by default; use --last to fork the
                    most recent)
  cloud             [EXPERIMENTAL] Browse tasks from Codex Cloud and apply changes locally
  exec-server       [EXPERIMENTAL] Run the standalone exec-server service
  features          Inspect feature flags
  help              Print this message or the help of the given subcommand(s)

Arguments:
  [PROMPT]
          Optional user prompt to start the session

Options:
  -c, --config <key=value>
          Override a configuration value that would otherwise be loaded from `~/.codex/config.toml`.
          Use a dotted path (`foo.bar.baz`) to override nested values. The `value` portion is parsed
          as TOML. If it fails to parse as TOML, the raw string is used as a literal.
          
          Examples: - `-c model="o3"` - `-c 'sandbox_permissions=["disk-full-read-access"]'` - `-c
          shell_environment_policy.inherit=all`

      --enable <FEATURE>
          Enable a feature (repeatable). Equivalent to `-c features.<name>=true`

      --disable <FEATURE>
          Disable a feature (repeatable). Equivalent to `-c features.<name>=false`

      --remote <ADDR>
          Connect the TUI to a remote app server endpoint.
          
          Accepted forms: `ws://host:port`, `wss://host:port`, `unix://`, or `unix://PATH`.

      --remote-auth-token-env <ENV_VAR>
          Name of the environment variable containing the bearer token to send to a remote app
          server websocket

      --strict-config
          Error out when config.toml contains fields that are not recognized by this version of
          Codex

  -i, --image <FILE>...
          Optional image(s) to attach to the initial prompt

  -m, --model <MODEL>
          Model the agent should use

      --oss
          Use open-source provider

      --local-provider <OSS_PROVIDER>
          Specify which local provider to use (lmstudio or ollama). If not specified with --oss,
          will use config default or show selection

  -p, --profile <CONFIG_PROFILE_V2>
          Layer $CODEX_HOME/<name>.config.toml on top of the base user config

  -s, --sandbox <SANDBOX_MODE>
          Select the sandbox policy to use when executing model-generated shell commands
          
          [possible values: read-only, workspace-write, danger-full-access]

      --approve-for-me
          Route approval requests through automatic review using the workspace-write sandbox

      --dangerously-bypass-approvals-and-sandbox
          Skip all confirmation prompts and execute commands without sandboxing. EXTREMELY
          DANGEROUS. Intended solely for running in environments that are externally sandboxed

      --dangerously-bypass-hook-trust
          Run enabled hooks without requiring persisted hook trust for this invocation. DANGEROUS.
          Intended only for automation that already vets hook sources

  -C, --cd <DIR>
          Tell the agent to use the specified directory as its working root

      --worktree
          Run the session in a new managed Git worktree

      --add-dir <DIR>
          Additional directories that should be writable alongside the primary workspace

  -a, --ask-for-approval <APPROVAL_POLICY>
          Configure when the model requires human approval before executing a command

          Possible values:
          - on-request: The model decides when to ask the user for approval
          - never:      Never ask for user approval Execution failures are immediately returned to
            the model

      --search
          Enable live web search. When enabled, the native Responses `web_search` tool is available
          to the model (no per‑call approval)

      --no-alt-screen
          Disable alternate screen mode
          
          Runs the TUI in inline mode, preserving terminal scrollback history.

      --no-daemon
          Run without the shared background server, even if it is already running

  -h, --help
          Print help (see a summary with '-h')

  -V, --version
          Print version
```

### `btmux skill`

Set up coding agents (Claude Code, Codex) to use btmux: a skill that teaches them its CLI, and a hook that tells them which pane they run in. Both take their text from the installed btmux when they run

```text
Usage: btmux skill <COMMAND>

Commands:
  setup   Set up the agents found here (Claude Code, Codex, Gemini CLI), or those given with --agent: hooks that report their state to btmux, and for Claude Code and Codex the btmux skill and a hook telling them their pane. With no agent found, the skill alone goes into ~/.agents/skills, which most agents read. Safe to run again; it updates what an older btmux installed
  status  Report what is installed for each agent
  remove  Remove what setup installed, leaving a skill btmux didn't install and every other hook alone
  show    Print the guide the skill gives agents
  help    Print this message or the help of the given subcommand(s)
```

#### `btmux skill setup`

Set up the agents found here (Claude Code, Codex, Gemini CLI), or those given with --agent: hooks that report their state to btmux, and for Claude Code and Codex the btmux skill and a hook telling them their pane. With no agent found, the skill alone goes into ~/.agents/skills, which most agents read. Safe to run again; it updates what an older btmux installed

```text
Usage: btmux skill setup [OPTIONS]

Options:
      --agent <AGENT>  Only this agent. Repeatable [possible values: claude, codex, gemini]
      --dry-run        Report what would change without changing anything
      --force          Replace a skill named btmux that btmux didn't install (moved to the trash)
      --json           Print the result as JSON
```

#### `btmux skill status`

Report what is installed for each agent

```text
Usage: btmux skill status [OPTIONS]

Options:
      --json  Print the result as JSON
```

#### `btmux skill remove`

Remove what setup installed, leaving a skill btmux didn't install and every other hook alone

```text
Usage: btmux skill remove [OPTIONS]

Options:
      --agent <AGENT>  Only this agent. Repeatable [possible values: claude, codex, gemini]
      --dry-run        Report what would change without changing anything
      --json           Print the result as JSON
```

#### `btmux skill show`

Print the guide the skill gives agents

```text
Usage: btmux skill show
```

### `btmux agents`

Print the panes running a coding agent, with their states, as JSON. Inside a btmux pane this targets that pane's server; otherwise the `--profile` server

```text
Usage: btmux agents
```

### `btmux wait`

Wait until the agent in PANE is idle, done, or blocked (or a `--state`), then print its status as JSON. Exits 2 on timeout

```text
Usage: btmux wait [OPTIONS] <PANE>

Arguments:
  <PANE>  Pane ID, as `btmux agents` lists it

Options:
      --state <STATES>     Agent state to wait for: idle, working, waiting, blocked, done, or unknown. Repeatable
      --timeout <TIMEOUT>  Seconds to wait, at most 3600 [default: 600]
```

### `btmux prompt`

Submit PROMPT (or stdin) to the agent in PANE, wait for its turn to finish, and print its status and screen as JSON. Exits 2 on timeout

```text
Usage: btmux prompt [OPTIONS] <PANE> [PROMPT]

Arguments:
  <PANE>    Pane ID, as `btmux agents` lists it
  [PROMPT]  Prompt text; read from stdin if omitted or `-`

Options:
      --no-wait            Return once submitted instead of waiting for the turn
      --timeout <TIMEOUT>  Seconds to wait, at most 3600 [default: 600]
```

### `btmux events`

Print server events as they happen, one line each, until interrupted: sessions, windows, and panes created, renamed, selected, or closed; agent state changes; notifications; widgets and popups. Inside a btmux pane this targets that pane's server; otherwise the `--profile` server. Reconnects after the server restarts or upgrades. Sandboxed panes can't use it

```text
Usage: btmux events [OPTIONS]

Options:
  -s, --session <SESSION>  Only this session's events (ID or name), exiting once it closes
      --json               Print each event as one line of JSON
```

## From a pane

### `btmux open-editor`

Open FILE (or just the editor), jumping to LINE. In a btmux pane, this opens the `editor` setting's editor over the pane; with `editor = "pane"`, or outside btmux, `$EDITOR` (falling back to `vi`)

```text
Usage: btmux open-editor [OPTIONS] [FILE] [LINE]

Arguments:
  [FILE]
          

  [LINE]
          

Options:
      --kind <KIND>
          The editor to use instead of the `editor` setting

          Possible values:
          - native: btmux's own editor
          - pane:   The pane's foreground Neovim, or `$EDITOR` in its shell (for `prefix + [`, in a popup)
          - neovim: The built-in Neovim
          - vscode: VS Code

  -h, --help
          Print help (see a summary with '-h')
```

### `btmux popup`

Run a program in a popup over this pane, closed when it exits, and exit with its status. Outside btmux, or if no tab shows the pane, it runs here instead

```text
Usage: btmux popup [OPTIONS] <COMMAND>...

Arguments:
  <COMMAND>...  The program and its arguments. A single argument is a command line for the shell

Options:
  -d, --cwd <CWD>         Working directory; the current one by default
  -T, --title <TITLE>     Title above the popup; the command by default
  -e, --env <NAME=VALUE>  Set an environment variable, as NAME=VALUE. Repeatable
  -t <TARGET>             The pane to open over, as `btmux list-panes` names it; this one by default
```

### `btmux pick`

Show a menu over this pane and print the chosen item. Items are stdin's lines; a tab starts a description, which is shown but not printed. Exits 1 with no items, 2 if no tab shows the pane, and 130 if canceled

```text
Usage: btmux pick [OPTIONS]

Options:
      --prompt <PROMPT>  Placeholder for the menu's filter
  -q, --query <QUERY>    The filter's initial text
  -m, --multi            Let Tab mark several items, printed in input order
      --no-sort          Keep input order while filtering, rather than ranking label prefixes first
      --read0            Items end in NUL rather than a newline and are taken whole, with no descriptions
      --print0           End printed items in NUL rather than a newline
  -1, --select-1         Print the item without a menu if only one matches the query
  -0, --exit-0           Exit 1 without a menu if nothing matches the query
```

### `btmux fzf`

fzf is an interactive filter program for any kind of list. It implements a "fuzzy" matching algorithm, so you can quickly type in patterns with omitted characters and still get the results you want. Project URL: https://github.com/junegunn/fzf Author: Junegunn Choi <junegunn.c@gmail.com> * See man page for more information: fzf --man

```text
Usage: fzf [options]

  SEARCH
    -e, --exact              Enable exact-match
    +x, --no-extended        Disable extended-search mode
    -i, --ignore-case        Case-insensitive match
    +i, --no-ignore-case     Case-sensitive match
        --smart-case         Smart-case match (default)
    --scheme=SCHEME          Scoring scheme [default|path|history]
    -n, --nth=N[,..]         Comma-separated list of field index expressions
                             for limiting search scope. Each can be a non-zero
                             integer or a range expression ([BEGIN]..[END]).
    --with-nth=N[,..]        Transform the presentation of each line using
                             field index expressions
    --accept-nth=N[,..]      Define which fields to print on accept
    -d, --delimiter=STR      Field delimiter regex (default: AWK-style)
    +s, --no-sort            Do not sort the result
    --literal                Do not normalize latin script letters
    --tail=NUM               Maximum number of items to keep in memory
    --disabled               Do not perform search
    --tiebreak=CRI[,..]      Comma-separated list of sort criteria to apply
                             when the scores are tied
                             [length|chunk|pathname|begin|end|index] (default: length)

  INPUT/OUTPUT
    --read0                  Read input delimited by ASCII NUL characters
    --print0                 Print output delimited by ASCII NUL characters
    --ansi                   Enable processing of ANSI color codes
    --sync                   Synchronous search for multi-staged filtering

  GLOBAL STYLE
    --style=PRESET           Apply a style preset [default|minimal|full[:BORDER_STYLE]
    --color=COLSPEC          Base scheme (dark|light|base16|bw) and/or custom colors
    --no-color               Disable colors
    --no-bold                Do not use bold text

  DISPLAY MODE
    --height=[~][-]HEIGHT[%] Display fzf window below the cursor with the given
                             height instead of using fullscreen.
                             A negative value is calculated as the terminal height
                             minus the given value.
                             If prefixed with '~', fzf will determine the height
                             according to the input size.
    --min-height=HEIGHT[+]   Minimum height when --height is given as a percentage.
                             Add '+' to automatically increase the value
                             according to the other layout options (default: 10+).
    --popup[=OPTS]           Start fzf in a floating pane (requires tmux 3.3+ or Zellij 0.44+)
                             [center|top|bottom|left|right][,SIZE[%]][,SIZE[%]]
                             [,border-native] (default: center,50%)
    --tmux[=OPTS]            Alias for --popup

  LAYOUT
    --layout=LAYOUT          Choose layout: [default|reverse|reverse-list]
    --margin=MARGIN          Screen margin (TRBL | TB,RL | T,RL,B | T,R,B,L)
    --padding=PADDING        Padding inside border (TRBL | TB,RL | T,RL,B | T,R,B,L)
    --border[=STYLE]         Draw border around the finder
                             [rounded|sharp|bold|block|thinblock|double|dashed|horizontal|vertical|
                              top|bottom|left|right|line|none] (default: rounded)
    --border-label=LABEL     Label to print on the border
    --border-label-pos=COL   Position of the border label
                             [POSITIVE_INTEGER: columns from left|
                              NEGATIVE_INTEGER: columns from right][:bottom]
                             (default: 0 or center)

  LIST SECTION
    -m, --multi[=MAX]        Enable multi-select with tab/shift-tab
    --highlight-line         Highlight the whole current line
    --cycle                  Enable cyclic scroll
    --wrap[=MODE]            Enable line wrap (char|word, default: char)
    --wrap-sign=STR          Indicator for wrapped lines
    --no-multi-line          Disable multi-line display of items when using --read0
    --raw                    Enable raw mode (show non-matching items)
    --track                  Track the current selection when the result is updated
    --id-nth=N[,..]          Define item identity fields for cross-reload operations
    --tac                    Reverse the order of the input
    --gap[=N]                Render empty lines between each item
    --gap-line[=STR]         Draw horizontal line on each gap using the string
                             (default: '┈' or '-')
    --freeze-left=N          Number of fields to freeze on the left
    --freeze-right=N         Number of fields to freeze on the right
    --keep-right             Keep the right end of the line visible on overflow
    --scroll-off=LINES       Number of screen lines to keep above or below when
                             scrolling to the top or to the bottom (default: 0)
    --no-hscroll             Disable horizontal scroll
    --hscroll-off=COLS       Number of screen columns to keep to the right of the
                             highlighted substring (default: 10)
    --jump-labels=CHARS      Label characters for jump mode
    --gutter=CHAR            Character used for the gutter column (default: '▌')
    --gutter-raw=CHAR        Character used for the gutter column in raw mode (default: '▖')
    --pointer=STR            Pointer to the current line (default: '▌' or '>')
    --marker=STR             Multi-select marker (default: '┃' or '>')
    --marker-multi-line=STR  Multi-select marker for multi-line entries;
                             3 elements for top, middle, and bottom (default: '╻┃╹')
    --ellipsis=STR           Ellipsis to show when line is truncated (default: '··')
    --tabstop=SPACES         Number of spaces for a tab character (default: 8)
    --scrollbar[=C1[C2]]     Scrollbar character(s)
                             (each for list section and preview window)
    --no-scrollbar           Hide scrollbar
    --list-border[=STYLE]    Draw border around the list section
                             [rounded|sharp|bold|block|thinblock|double|dashed|horizontal|vertical|
                              top|bottom|left|right|none] (default: rounded)
    --list-label=LABEL       Label to print on the list border
    --list-label-pos=COL     Position of the list label
                             [POSITIVE_INTEGER: columns from left|
                              NEGATIVE_INTEGER: columns from right][:bottom]
                             (default: 0 or center)

  INPUT SECTION
    --no-input               Disable and hide the input section
    --prompt=STR             Input prompt (default: '> ')
    --info=STYLE             Finder info style
                             [default|right|hidden|inline[-right][:PREFIX]]
    --info-command=COMMAND   Command to generate info line
    --separator=STR          Draw horizontal separator on info line using the string
                             (default: '─' or '-')
    --no-separator           Hide info line separator
    --ghost=TEXT             Ghost text to display when the input is empty
    --filepath-word          Make word-wise movements respect path separators
    --input-border[=STYLE]   Draw border around the input section
                             [rounded|sharp|bold|block|thinblock|double|dashed|horizontal|vertical|
                              top|bottom|left|right|line|none] (default: rounded)
    --input-label=LABEL      Label to print on the input border
    --input-label-pos=COL    Position of the input label
                             [POSITIVE_INTEGER: columns from left|
                              NEGATIVE_INTEGER: columns from right][:bottom]
                             (default: 0 or center)

  PREVIEW WINDOW
    --preview=COMMAND        Command to preview highlighted line ({})
    --preview-window=OPT     Preview window layout (default: right:50%)
                             [up|down|left|right|next][,SIZE[%]]
                             [,[no]wrap[-word]][,[no]cycle][,[no]follow][,[no]info]
                             [,[no]hidden][,border-STYLE]
                             [,+SCROLL[OFFSETS][/DENOM]][,~HEADER_LINES]
                             [,default][,<SIZE_THRESHOLD(ALTERNATIVE_LAYOUT)]
    --preview-border[=STYLE] Short for --preview-window=border-STYLE
                             [rounded|sharp|bold|block|thinblock|double|dashed|horizontal|vertical|
                              top|bottom|left|right|line|none] (default: rounded)
    --preview-label=LABEL
    --preview-label-pos=N    Same as --border-label and --border-label-pos,
                             but for preview window
    --preview-wrap-sign=STR  Indicator for wrapped lines in the preview window

  HEADER
    --header=STR             String to print as header
    --header-lines=N         The first N lines of the input are treated as header
    --header-first           Print header before the prompt line
    --header-border[=STYLE]  Draw border around the header section
                             [rounded|sharp|bold|block|thinblock|double|dashed|horizontal|vertical|
                              top|bottom|left|right|line|inline|none] (default: rounded)
    --header-lines-border[=STYLE]
                             Display header from --header-lines with a separate border.
                             Pass 'none' to still separate it but without a border.
                             Pass 'inline' to embed it inside the list frame.
    --header-label=LABEL     Label to print on the header border
    --header-label-pos=COL   Position of the header label
                             [POSITIVE_INTEGER: columns from left|
                              NEGATIVE_INTEGER: columns from right][:bottom]
                             (default: 0 or center)

  FOOTER
    --footer=STR             String to print as footer
    --footer-border[=STYLE]  Draw border around the footer section
                             [rounded|sharp|bold|block|thinblock|double|dashed|horizontal|vertical|
                              top|bottom|left|right|line|inline|none] (default: line)
    --footer-label=LABEL     Label to print on the footer border
    --footer-label-pos=COL   Position of the footer label
                             [POSITIVE_INTEGER: columns from left|
                              NEGATIVE_INTEGER: columns from right][:bottom]
                             (default: 0 or center)

  SCRIPTING
    -q, --query=STR          Start the finder with the given query
    -1, --select-1           Automatically select the only match
    -0, --exit-0             Exit immediately when there's no match
    -f, --filter=STR         Print matches for the initial query and exit
    --print-query            Print query as the first line
    --expect=KEYS            Comma-separated list of keys to complete fzf

  KEY/EVENT BINDING
    --bind=BINDINGS          Custom key/event bindings

  ADVANCED
    --with-shell=STR         Shell command and flags to start child processes with
    --listen[=[ADDR:]PORT]   Start HTTP server to receive actions via TCP
                             (To allow remote process execution, use --listen-unsafe)
    --listen=SOCKET_PATH     Start HTTP server to receive actions via Unix domain socket
                             (Path should end with .sock)

  DIRECTORY TRAVERSAL        (Only used when $FZF_DEFAULT_COMMAND is not set)
    --walker=OPTS            [file][,dir][,follow][,hidden] (default: file,follow,hidden)
    --walker-root=DIR [...]  List of directories to walk (default: .)
    --walker-skip=DIRS       Comma-separated list of directory names to skip
                             (default: .git,node_modules)

  HISTORY
    --history=FILE           File to store fzf search history (*not* shell command history)
    --history-size=N         Maximum number of entries to keep in the file (default: 1000)

  SHELL INTEGRATION
    --bash                   Print script to set up Bash shell integration
    --zsh                    Print script to set up Zsh shell integration
    --fish                   Print script to set up Fish shell integration
    --nushell                Print script to set up Nushell integration

  HELP
    --version                Display version information and exit
    --help                   Show this message
    --man                    Show man page

  ENVIRONMENT VARIABLES
    FZF_DEFAULT_COMMAND      Default command to use when input is tty
    FZF_DEFAULT_OPTS         Default options (e.g. '--layout=reverse --info=inline')
    FZF_DEFAULT_OPTS_FILE    Location of the file to read default options from
    FZF_API_KEY              X-API-Key header for HTTP server (--listen)
```

### `btmux confirm`

Ask a yes/no question over this pane. Exits 0 for yes, 1 for no, 2 if no tab shows the pane, and 130 if canceled

```text
Usage: btmux confirm [OPTIONS] <MESSAGE>

Arguments:
  <MESSAGE>  The question

Options:
      --default-yes  Make Enter answer yes rather than no
```

### `btmux input`

Ask for a line of text over this pane and print it. Exits 2 if no tab shows the pane and 130 if canceled

```text
Usage: btmux input [OPTIONS]

Options:
      --prompt <PROMPT>    Label above the text field
      --default <DEFAULT>  Text the field starts with
```

### `btmux send-notification`

Show a notification in every connected browser. Inside a btmux pane this targets that pane's server; otherwise the `--profile` server

```text
Usage: btmux send-notification [OPTIONS] <TITLE>

Arguments:
  <TITLE>  Notification title

Options:
      --body <BODY>    Optional body text
      --level <LEVEL>  [default: info] [possible values: info, attention, success, error]
```

## Sessions, windows, and panes

### `btmux list-sessions`

List sessions as JSON, or one line each with a -F format

```text
Usage: btmux list-sessions [OPTIONS]

Options:
  -F <FORMAT>  Line format, e.g. '#{session_name}'
```

### `btmux list-windows`

List the target session's windows (-a: all) as JSON, or with -F

```text
Usage: btmux list-windows [OPTIONS]

Options:
  -a           
  -t <TARGET>  A pane, window, or session ID, or SESSION[:WINDOW[.PANE]] by name or index; an empty part is the current one (:1, :.2). Default: this pane
  -F <FORMAT>
```

### `btmux list-panes`

List the target window's panes (-s: session, -a: all) as JSON, or with -F

```text
Usage: btmux list-panes [OPTIONS]

Options:
  -a           
  -s           
  -t <TARGET>  A pane, window, or session ID, or SESSION[:WINDOW[.PANE]] by name or index; an empty part is the current one (:1, :.2). Default: this pane
  -F <FORMAT>
```

### `btmux new-session`

Create a session and print its pane's ID

```text
Usage: btmux new-session [OPTIONS]

Options:
  -s <NAME>         
  -n <WINDOW_NAME>  Window name
  -c <CWD>          Working directory
  -F <FORMAT>       Print with this format instead of the pane ID
```

### `btmux new-window`

Create a window in the target session and print its pane's ID. With COMMAND, its pane runs that instead of the shell and closes when it exits (unless --keep-open)

```text
Usage: btmux new-window [OPTIONS] [COMMAND]...

Arguments:
  [COMMAND]...  The program and its arguments, run now; a single argument is a command line for the shell. Default: the shell

Options:
  -t <TARGET>      A pane, window, or session ID, or SESSION[:WINDOW[.PANE]] by name or index; an empty part is the current one (:1, :.2). Default: this pane
  -n <NAME>        
  -c <CWD>         Working directory; with COMMAND, the current one by default
  -d               Don't make it the active window
  -F <FORMAT>      Print with this format instead of the pane ID
  -e <NAME=VALUE>  Set an environment variable for COMMAND, as NAME=VALUE. Repeatable
      --keep-open  Keep the pane, with its output, after its program exits
      --wait       After printing, wait for the program to exit and exit with its status, as `wait-exit` does
```

### `btmux split-window`

Split the target pane and print the new pane's ID. With COMMAND, the new pane runs that instead of the shell and closes when it exits (unless --keep-open)

```text
Usage: btmux split-window [OPTIONS] [COMMAND]...

Arguments:
  [COMMAND]...  The program and its arguments, run now; a single argument is a command line for the shell. Default: the shell

Options:
  -t <TARGET>      A pane, window, or session ID, or SESSION[:WINDOW[.PANE]] by name or index; an empty part is the current one (:1, :.2). Default: this pane
  -h               Side by side (new pane on the right)
  -v               Stacked (new pane below); the default
  -b               Put the new pane left or above
  -d               Don't make the new pane active
  -c <CWD>         Working directory; default the target pane's, or with COMMAND the current one
  -F <FORMAT>      Print with this format instead of the pane ID
      --help       
  -e <NAME=VALUE>  Set an environment variable for COMMAND, as NAME=VALUE. Repeatable
      --keep-open  Keep the pane, with its output, after its program exits
      --wait       After printing, wait for the program to exit and exit with its status, as `wait-exit` does
```

### `btmux kill-pane`

Kill the target pane. Its window closes with its last pane, and the session with its last window

```text
Usage: btmux kill-pane [OPTIONS]

Options:
  -t <TARGET>  A pane, window, or session ID, or SESSION[:WINDOW[.PANE]] by name or index; an empty part is the current one (:1, :.2). Default: this pane
```

### `btmux kill-window`

Kill the target window, and the session with its last window

```text
Usage: btmux kill-window [OPTIONS]

Options:
  -t <TARGET>  A pane, window, or session ID, or SESSION[:WINDOW[.PANE]] by name or index; an empty part is the current one (:1, :.2). Default: this pane
```

### `btmux kill-session`

Kill the target session

```text
Usage: btmux kill-session [OPTIONS]

Options:
  -t <TARGET>  A pane, window, or session ID, or SESSION[:WINDOW[.PANE]] by name or index; an empty part is the current one (:1, :.2). Default: this pane
```

### `btmux select-pane`

Make the target pane active, and its window

```text
Usage: btmux select-pane [OPTIONS]

Options:
  -t <TARGET>  A pane, window, or session ID, or SESSION[:WINDOW[.PANE]] by name or index; an empty part is the current one (:1, :.2). Default: this pane
```

### `btmux select-window`

Make the target window active

```text
Usage: btmux select-window [OPTIONS]

Options:
  -t <TARGET>  A pane, window, or session ID, or SESSION[:WINDOW[.PANE]] by name or index; an empty part is the current one (:1, :.2). Default: this pane
```

### `btmux rename-window`

Rename the target window

```text
Usage: btmux rename-window [OPTIONS] <NAME>

Arguments:
  <NAME>  

Options:
  -t <TARGET>  A pane, window, or session ID, or SESSION[:WINDOW[.PANE]] by name or index; an empty part is the current one (:1, :.2). Default: this pane
```

### `btmux rename-session`

Rename the target session

```text
Usage: btmux rename-session [OPTIONS] <NAME>

Arguments:
  <NAME>  

Options:
  -t <TARGET>  A pane, window, or session ID, or SESSION[:WINDOW[.PANE]] by name or index; an empty part is the current one (:1, :.2). Default: this pane
```

### `btmux send-keys`

Type into the target pane. Each argument is a key name (Enter, C-c, M-x, Up, F5, ...) or else literal text; no spaces are added between arguments

```text
Usage: btmux send-keys [OPTIONS] [KEYS]...

Arguments:
  [KEYS]...  

Options:
  -t <TARGET>  A pane, window, or session ID, or SESSION[:WINDOW[.PANE]] by name or index; an empty part is the current one (:1, :.2). Default: this pane
  -l           Send every argument as literal text
```

### `btmux capture-pane`

Print the target pane as rendered text, with wrapped lines joined

```text
Usage: btmux capture-pane [OPTIONS]

Options:
  -t <TARGET>  A pane, window, or session ID, or SESSION[:WINDOW[.PANE]] by name or index; an empty part is the current one (:1, :.2). Default: this pane
  -S <START>   History to include: '-' for all, -N for N lines above the screen
```

### `btmux wait-exit`

Wait for the target pane's program (its command, or its shell) to exit, and exit with its status: 128+N if signal N ended it, 1 if the pane closed before it reported one, 124 on timeout. A pane that is kept open, or closed in the last 5 minutes, reports the status it exited with

```text
Usage: btmux wait-exit [OPTIONS]

Options:
  -t <TARGET>              A pane ID (also of a closed pane) or tmux target. Default: this pane
      --timeout <SECONDS>  Seconds to wait; without it, wait until the program exits
```

### `btmux new-worktree`

Create a git worktree for BRANCH beside the target pane's repo (the `worktree-path` setting), open a window there in its session, and print its pane's ID. An existing local or remote branch is checked out; a new one starts at the target pane's HEAD

```text
Usage: btmux new-worktree [OPTIONS] <BRANCH>

Arguments:
  <BRANCH>  

Options:
  -t <TARGET>  A pane, window, or session ID, or SESSION[:WINDOW[.PANE]] by name or index; an empty part is the current one (:1, :.2). Default: this pane
  -d           Don't make it the active window
  -F <FORMAT>  Print with this format instead of the pane ID
```

### `btmux remove-worktree`

Remove the linked git worktree the target pane is in, close every pane in it, and print its path. Refuses uncommitted changes and panes running a program; the branch is kept

```text
Usage: btmux remove-worktree [OPTIONS]

Options:
  -t <TARGET>  A pane, window, or session ID, or SESSION[:WINDOW[.PANE]] by name or index; an empty part is the current one (:1, :.2). Default: this pane
```

### `btmux forward`

Listen on a local port and forward each connection to a TCP port that a program in the pane listens on, such as a dev server, until Ctrl+C. Prints the local URL. Exits 3 when the pane closes

```text
Usage: btmux forward [OPTIONS] [[ADDRESS:]PORT]

Arguments:
  [[ADDRESS:]PORT]  The pane's port, as PORT or ADDRESS:PORT ([::1]:PORT for IPv6) when different programs hold it on different addresses. Default: its only listening port

Options:
  -t <TARGET>                    The pane: an ID or SESSION[:WINDOW[.PANE]]. Default: this pane
      --bind <BIND>              Local address to listen on [default: 127.0.0.1]
      --local-port <LOCAL_PORT>  Local port. Default: the pane's port when it's free here and doesn't stand in the pane's way, else any
      --open                     Open the forwarded URL in the default browser
      --server <URL>             Server URL, such as https://machine.tailnet.ts.net:8004. The token is read from BTMUX_AUTH_TOKEN
```

## Setup

### `btmux completions`

Print shell completions to stdout

```text
Usage: btmux completions <SHELL>

Arguments:
  <SHELL>  Shell to generate completions for [possible values: bash, elvish, fish, powershell, zsh]
```

### `btmux generate-config`

Print a default config.toml with all settings documented and commented out

```text
Usage: btmux generate-config
```

### `btmux generate-claude-code-hooks`

Print a Claude Code hooks.json snippet for btmux pane notifications

```text
Usage: btmux generate-claude-code-hooks
```

### `btmux generate-codex-hooks`

Print a Codex hooks.json snippet for btmux pane notifications

```text
Usage: btmux generate-codex-hooks
```

### `btmux generate-gemini-cli-hooks`

Print a Gemini CLI settings.json hooks snippet for btmux pane notifications

```text
Usage: btmux generate-gemini-cli-hooks
```

### `btmux install-fish-integration`

Install the btmux fish theme and a conf.d snippet for btmux panes, which selects it and can make Tab complete through `btmux pick`

```text
Usage: btmux install-fish-integration
```

### `btmux generate-fish-theme`

Print a fish theme that follows the btmux color scheme

```text
Usage: btmux generate-fish-theme
```

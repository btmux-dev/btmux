---
title: "Comparison with other projects"
sidebar:
  order: 1
---

Data collected on November 17, 2026.

**GUI** = desktop/mobile app with an integrated terminal emulator;
**browser** = terminal emulator in a web page; **TUI** = client running inside
another terminal emulator. An automation CLI alone does not count as a TUI client.

**—** = not included; **?** = not established by the linked documentation.
Named clients and integrations may require extra installation or setup.

## Access and sessions

| Project                                                | App type      | Browser client                                                                                          | Phone/tablet access                                                              | Detached live sessions[^detached]                                              | Multiple clients on the same live session                  | Native Windows app/server                                                                                      |
| ------------------------------------------------------ | ------------- | ------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------- | ------------------------------------------------------------------------------ | ---------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------- |
| **btmux**                                              | GUI + browser | Yes                                                                                                     | Browser                                                                          | Yes                                                                            | Yes                                                        | Experimental                                                                                                   |
| [herdr TUI][herdr]                                     | TUI           | —                                                                                                       | SSH client                                                                       | Yes                                                                            | Yes                                                        | Yes                                                                                                            |
| [herdr app][herdr-app][^herdr-app]                     | GUI           | —                                                                                                       | [iOS / Android](https://herdr.dev/mobile/)                                       | Yes                                                                            | Yes                                                        | Yes                                                                                                            |
| [cmux][cmux][^cmux]                                    | GUI + TUI     | —                                                                                                       | iOS beta                                                                         | TUI server                                                                     | TUI daemon                                                 | TUI                                                                                                            |
| [Muxy][muxy]                                           | GUI + TUI     | —                                                                                                       | iOS / Android                                                                    | Yes                                                                            | Yes                                                        | [Beta](https://github.com/muxy-app/muxy/blob/main/docs/user-guide/getting-started.md#windows-desktop-app-beta) |
| [Paradev (formerly Superset)][paradev]                 | GUI           | ?                                                                                                       | [iOS / iPadOS (Pro)](https://docs.paradev.com/install#iphone-and-ipad)           | Yes                                                                            | [Yes](https://docs.paradev.com/multiple-windows)           | —                                                                                                              |
| [Orca][orca][^orca]                                    | GUI + browser | [Paired web client](https://orca.dev/docs/ways-to-run#3-remote-orca-servers)                            | [iOS / Android (beta)](https://orca.dev/docs/mobile)                             | [Yes](https://orca.dev/docs/model/session-restore)                             | [Remote Orca Server](https://orca.dev/docs/remote-servers) | Yes                                                                                                            |
| [TUIOS][tuios]                                         | Browser + TUI | [Separate `tuios-web`](https://tuios.dev/docs/web)                                                      | Browser                                                                          | Yes                                                                            | Yes                                                        | Yes                                                                                                            |
| [Emdash][emdash]                                       | GUI           | —                                                                                                       | —                                                                                | [Remote daemon][emdash-daemon] / [tmux](https://emdash.com/docs/tmux-sessions) | Remote daemon                                              | Yes                                                                                                            |
| [Sheprd (formerly herdr-gpui)][sheprd][^herdr-clients] | GUI + browser | [Phone web app](https://github.com/sheprd-labs/sheprd/blob/main/crates/sheprd-remote-control/README.md) | Browser / [iOS beta](https://sheprd.org/testflight/)                             | Via Herdr                                                                      | Yes                                                        | Yes                                                                                                            |
| [Ovis][ovis][^herdr-clients]                           | GUI + browser | Yes                                                                                                     | Browser / iOS / Android                                                          | Via Herdr                                                                      | Yes                                                        | Yes                                                                                                            |
| [Rune][rune]                                           | GUI + TUI     | —                                                                                                       | —                                                                                | ?                                                                              | ?                                                          | —                                                                                                              |
| [tty7][tty7]                                           | GUI           | —                                                                                                       | [iOS beta / Android](https://github.com/l0ng-ai/tty7/blob/main/mobile/README.md) | Yes                                                                            | Desktop + phone[^tty7]                                     | Yes                                                                                                            |
| [Wave][wave]                                           | GUI           | —                                                                                                       | —                                                                                | [SSH only](https://docs.waveterm.dev/durable-sessions)                         | —                                                          | Yes                                                                                                            |
| [Warp][warp]                                           | GUI           | Agent sessions[^warp-sharing]                                                                           | Browser[^warp-sharing]                                                           | —                                                                              | Agent sessions[^warp-sharing]                              | Yes                                                                                                            |

## Development tools and automation

| Project                | Agent status                                                                       | Git integration                                                                                                | Graphical code editor                                                                                           | Embedded web browser                          | Terminal automation                                                                                                | License                                                                                     | Agent sandbox[^sandbox]                                                                                                            |
| ---------------------- | ---------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------- | --------------------------------------------- | ------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------- |
| **btmux**              | [Detection / hooks / OSC 7501](../agents/detection.md)                | GUI, worktrees, PR/CI status[^git]                                                                             | [Built in / Neovim / VS Code](../features/editors.md)[^editors]                                                             | [macOS / Linux](../features/browser.md)[^browser]         | [CLI, REST, MCP](../automation/index.md)                                                                                    | [MIT](../../../../../LICENSE)                                                                           | [Multipass VM](../features/sandbox.md)                                                                                                         |
| [herdr TUI][herdr]     | Detection / hooks                                                                  | Worktree tracking                                                                                              | —                                                                                                               | —                                             | [CLI, socket API](https://herdr.dev/tui/docs/socket-api/)                                                          | [Apache-2.0](https://github.com/herdrdev/herdr/blob/master/LICENSE)                         | —                                                                                                                                  |
| [herdr app][herdr-app] | OSC 7501[^herdr-app]                                                               | Worktrees, repo status, diffs                                                                                  | Mod API only[^herdr-app]                                                                                        | —                                             | [CLI, core API](https://herdr.dev/core/)                                                                           | [Apache-2.0](https://github.com/herdrdev/herdr-app/blob/master/LICENSE)                     | —                                                                                                                                  |
| [cmux][cmux]           | [Agent hooks](https://cmux.com/docs/notifications)                                 | Branch/PR metadata, diffs                                                                                      | [Text editor (macOS)](https://cmux.com/docs/changelog/0.64.23)                                                  | Yes                                           | [CLI, socket API](https://cmux.com/docs/api)                                                                       | [GPL-3.0-or-later / BUSL-1.1](https://github.com/manaflow-ai/cmux/blob/main/LICENSE)[^cmux] | [Cloud VMs (early access)](https://cmux.com/docs/cloud)                                                                            |
| [Muxy][muxy]           | [Detection / OSC 7501](https://muxy.app/docs/2.x/features/ai-agents)               | [GUI, worktrees, PR/CI status](https://github.com/muxy-app/muxy/blob/main/docs/features/git.md)                | [Files extension](https://muxy.app/store/files)                                                                 | macOS / Linux                                 | [CLI, socket API](https://muxy.app/docs/2.x/features/muxy-cli)                                                     | [Apache-2.0](https://github.com/muxy-app/muxy/blob/main/LICENSE)                            | —                                                                                                                                  |
| [Paradev][paradev]     | [Agent integration](https://docs.paradev.com/agent-integration)                    | [GUI, worktrees, PR/CI status](https://docs.paradev.com/pull-requests)                                         | [Built in](https://docs.paradev.com/editor)                                                                     | Yes                                           | [CLI, API, MCP](https://docs.paradev.com/mcp-server)                                                               | [MIT](https://github.com/ParadevAI/Paradev/blob/main/LICENSE.md)                            | [Cloud (paid plans)](https://docs.paradev.com/cloud-workspaces)                                                                    |
| [Orca][orca]           | [Agent status hooks](https://orca.dev/docs/agents/hooks-memory#agent-status-hooks) | [GUI, worktrees, PR/CI status](https://orca.dev/docs/review/github)                                            | [Monaco](https://orca.dev/docs/editing/monaco)                                                                  | [Yes](https://orca.dev/docs/browser/overview) | [`orca` CLI](https://orca.dev/docs/cli/overview)                                                                   | [MIT](https://github.com/stablyai/orca/blob/main/LICENSE)                                   | [Per-workspace VMs](https://orca.dev/docs/ways-to-run#4-cloud-vms-per-workspace-environments)                                      |
| [TUIOS][tuios]         | [Detection / hooks / OSC 7501](https://tuios.dev/docs/agents)                      | [Worktrees](https://tuios.dev/docs/worktrees), [review, PR/CI status](https://tuios.dev/docs/ship)             | —                                                                                                               | —                                             | [CLI, JSON protocol](https://tuios.dev/docs/control-protocol), MCP                                                 | [MIT](https://github.com/Gaurav-Gosain/tuios/blob/main/LICENSE)                             | —                                                                                                                                  |
| [Emdash][emdash]       | Agent lifecycle hooks                                                              | [GUI, worktrees, PR/CI status](https://emdash.com/docs/ci-checks)                                              | [Built in](https://emdash.com/docs/file-editor)                                                                 | [Yes](https://emdash.com/docs/in-app-browser) | [Workspace API](https://github.com/generalaction/emdash/blob/main/apps/workspace-server/docs/client-connection.md) | [Apache-2.0](https://github.com/generalaction/emdash/blob/main/LICENSE.md)                  | —                                                                                                                                  |
| [Sheprd][sheprd]       | Herdr backend                                                                      | GUI, worktrees, PR status                                                                                      | [VS Code](https://github.com/sheprd-labs/sheprd/blob/main/crates/sheprd-gpui/README.md#vs-code)[^herdr-clients] | macOS / Windows                               | `sheprd` CLI, Herdr API                                                                                            | [Apache-2.0](https://github.com/sheprd-labs/sheprd/blob/main/LICENSE)                       | [Containers / cloud VMs](https://github.com/sheprd-labs/sheprd/blob/main/crates/sheprd-gpui/README.md#containers-docker-or-podman) |
| [Ovis][ovis]           | Herdr backend                                                                      | Read-only diffs                                                                                                | Preview only                                                                                                    | —                                             | Herdr API                                                                                                          | [MIT](https://github.com/devswha/ovis/blob/main/LICENSE)                                    | —                                                                                                                                  |
| [Rune][rune]           | [OSC 7501](https://docs.rune.build/learn/terminal/program-status/)                 | Worktrees, diffs                                                                                               | Built in                                                                                                        | —                                             | `runectl` CLI                                                                                                      | [GPL-3.0-or-later](https://github.com/unstablebuild/rune/blob/main/LICENSE)                 | —                                                                                                                                  |
| [tty7][tty7]           | Agent detection / status                                                           | [GUI, worktrees, PR/CI status](https://github.com/l0ng-ai/tty7/blob/main/docs/git/source-control.mdx)          | [Built in](https://github.com/l0ng-ai/tty7/blob/main/docs/window/side-panel.mdx#the-editor)                     | —                                             | CLI                                                                                                                | [Apache-2.0](https://github.com/l0ng-ai/tty7/blob/main/LICENSE)                             | —                                                                                                                                  |
| [Wave][wave]           | [Claude Code hooks](https://docs.waveterm.dev/claude-code)[^wave-status]           | —                                                                                                              | Built in                                                                                                        | Yes                                           | [`wsh` CLI](https://docs.waveterm.dev/wsh)                                                                         | [Apache-2.0](https://github.com/wavetermdev/waveterm/blob/main/LICENSE)                     | —                                                                                                                                  |
| [Warp][warp]           | [CLI agent integration](https://docs.warp.dev/agents/cli-agents/overview/)         | [Code review](https://docs.warp.dev/code/code-review/), [worktrees](https://docs.warp.dev/code/git-worktrees/) | Built in                                                                                                        | —                                             | [`warpctrl` CLI](https://docs.warp.dev/changelog/2026/)                                                            | [AGPL-3.0 / MIT](https://github.com/warpdotdev/warp#licensing)                              | [Docker](https://docs.warp.dev/terminal/settings/all-settings/) / [cloud](https://docs.warp.dev/factories/warp-hosting/)           |

[^detached]:
    Running shells and programs survive the client disconnecting or exiting
    while their server stays running. Restoring layouts, scrollback, or an
    agent conversation after a restart is a separate feature.

[^sandbox]:
    Shells or agents run in a VM or container that the project creates and
    manages, locally or with a cloud provider. Machines that are only reachable
    over SSH don't count.

[^herdr-app]:
    herdr-app uses herdr-core, separate from herdr TUI's server. Agent status
    requires programs to report OSC 7501. Its mod API exposes editor widgets;
    the published [Code layout](https://herdr.dev/layouts/herdrdev/code/) is a
    read-only example. Its iOS and Android apps reach herdr-core over SSH; they
    don't support herdr TUI.

[^cmux]:
    cmux has a macOS app and a separate [cross-platform TUI](https://cmux.com/docs/tui).
    Detached sessions, multiple clients, and native Windows support here refer
    to the TUI and its [daemon](https://github.com/manaflow-ai/cmux/blob/main/cmux-tui/docs/remote.md).
    Apps and the TUI use GPL-3.0-or-later; the server components listed in cmux's
    license use BUSL-1.1. The macOS app can keep terminals alive through opt-in
    tmux (`cmux local-tmux`).

[^orca]:
    A background daemon keeps local terminals running after the app quits. A
    Remote Orca Server (the desktop app or `orca serve`) shares its sessions
    with desktop, web, and mobile clients; only the desktop app serves the web
    client. The mobile app is a companion for monitoring and steering agents,
    not a full client. Orca's Monaco editor has completions and diagnostics
    only for TypeScript, JavaScript, JSON, CSS, SCSS, Less, and HTML, and runs
    no language servers.

[^herdr-clients]:
    Sheprd (formerly herdr-gpui) and Ovis are separate clients of Herdr
    backends, and both also run herdr-core terminals; Ovis's desktop app runs
    its own. Sheprd's VS Code tab embeds a separate VS Code server; its embedded
    web views are available on macOS and Windows. Its phone remote control is a
    paired web app plus an iPhone/iPad TestFlight beta.
    [Ovis's guide](https://github.com/devswha/ovis/blob/main/docs/guide.md)
    describes its shared terminal views and pane splitting.

[^tty7]:
    tty7's phone app views and types into the desktop's live panes beside the
    desktop window. Desktop clients take over a workspace one at a time.

[^editors]:
    btmux's own editor is shared live by every client, with language servers,
    Git signs, and Vim mode. It can also render a Neovim GUI or embed VS Code;
    those must be installed separately. Terminal editors can run in every
    project listed.

[^git]:
    btmux shows each pane branch's pull request and CI checks from GitHub,
    through the `gh` CLI.

[^browser]:
    btmux shows a Chrome or Chromium running headless on its server, so the page
    works in every client and `localhost` means the server. The browser must be
    installed separately.

[^wave-status]:
    Wave's official Claude Code integration uses `wsh badge` and configured
    lifecycle hooks to show permission requests, questions, and completed turns.

[^warp-sharing]:
    Warp's [agent session sharing](https://docs.warp.dev/agents/local-agents/session-sharing/)
    and [Remote Control](https://docs.warp.dev/agents/cli-agents/remote-control)
    let browser and mobile clients view and steer agent sessions through Warp's
    cloud service. The desktop app must remain running for local execution.

[herdr]: https://github.com/herdrdev/herdr
[herdr-app]: https://github.com/herdrdev/herdr-app
[cmux]: https://github.com/manaflow-ai/cmux
[muxy]: https://github.com/muxy-app/muxy
[paradev]: https://github.com/ParadevAI/Paradev
[orca]: https://github.com/stablyai/orca
[tuios]: https://github.com/Gaurav-Gosain/tuios
[emdash]: https://github.com/generalaction/emdash
[emdash-daemon]: https://github.com/generalaction/emdash/blob/main/apps/workspace-server/docs/daemon.md
[sheprd]: https://github.com/sheprd-labs/sheprd
[ovis]: https://github.com/devswha/ovis
[rune]: https://github.com/unstablebuild/rune
[tty7]: https://github.com/l0ng-ai/tty7
[wave]: https://github.com/wavetermdev/waveterm
[warp]: https://github.com/warpdotdev/warp

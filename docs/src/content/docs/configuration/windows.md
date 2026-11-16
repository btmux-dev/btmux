---
title: "Windows"
sidebar:
  order: 6
---

:::caution
Windows support (x64 and ARM64) is experimental.
:::

## Paths

| File         | Location                              |
| ------------ | ------------------------------------- |
| Config       | `%APPDATA%\btmux\config.toml`         |
| Access token | `%LOCALAPPDATA%\btmux\state.token`    |
| Saved state  | `%LOCALAPPDATA%\btmux\state.json`     |

## Shells

Panes run `powershell.exe` by default. Set `shell` in `config.toml` to another
command line:

```toml
shell = "pwsh"
# shell = "cmd.exe"
# shell = 'C:\Program Files\Git\bin\bash.exe'   # Git Bash
# shell = "wsl.exe -d Ubuntu"
```

Under bash, btmux tracks the pane's directory with `PROMPT_COMMAND` and `PS0`.
If your bashrc replaces them, emit OSC 7 and OSC 133 A/C marks yourself.

## What's missing

- No background service: `btmux install` exits with an error. Run `btmux` in a
  terminal, or use the desktop app.
- No in-place upgrade. To update, quit btmux and the desktop app, replace
  `btmux.exe` or run the new installer, and start it again. Running shells end;
  sessions, windows, and pane directories are restored.
- No [VS Code](../features/vscode.md), [browser](../features/browser.md), or
  [sandboxed sessions](../features/sandbox.md).
- The [process viewer](../features/process-viewer.md) doesn't count programs
  in Git Bash and WSL panes.

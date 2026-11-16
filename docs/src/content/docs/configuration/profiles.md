---
title: "Profiles and persistence"
sidebar:
  order: 2
---

By default, session structure is saved to
`~/.local/state/btmux/state.json` (respecting `$XDG_STATE_HOME`). Named profiles
let you run isolated instances, each with separate state:

```sh
btmux --profile work --port 8005
btmux --profile personal --port 8006
```

Profile state is stored under `$XDG_STATE_HOME/btmux/<profile>/state.json`, or
`~/.local/state/btmux/<profile>/state.json` when `XDG_STATE_HOME` is unset. The
default profile continues to use the top-level `state.json`.

Persistence covers the session/window/pane tree, layouts, names, active
indices, and each pane's last-known working directory. Running shell processes
and scrollback do not survive a btmux restart. Restored panes lazily start fresh
shells in their saved working directories. `btmux upgrade` moves a running
server to a new binary without a restart, keeping both.

A file lock keeps two instances from using the same profile. Each profile has
its own [access token](access.md#token-files).

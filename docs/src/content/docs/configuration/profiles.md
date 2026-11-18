---
title: "Profiles and persistence"
sidebar:
  order: 5
---

## What is saved

btmux saves its sessions, windows, pane layouts, names, and each pane's
directory to a state file:

```text
~/.local/state/btmux/state.json
```

The state file follows `$XDG_STATE_HOME`. After a restart, panes start fresh
shells in their saved directories.
[`btmux upgrade`](../internals/upgrade.md) preserves running programs and
scrollback.

## Profiles

A profile is a separate server with its own sessions, state file, and
[access token](access.md#token-files):

```sh
btmux --profile work --port 8005
btmux --profile personal --port 8006
```

A profile's files are in `~/.local/state/btmux/<profile>/`. Only one server
can run a profile at a time.

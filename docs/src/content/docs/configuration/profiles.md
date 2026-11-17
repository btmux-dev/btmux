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

It follows `$XDG_STATE_HOME`. Running programs and scrollback are not saved:
after a restart, each pane starts a fresh shell in its saved directory.
[`btmux upgrade`](../internals/upgrade.md) keeps both, by moving the running
server to a new binary without a restart.

## Profiles

A profile is a separate server with its own sessions, state file, and
[access token](access.md#token-files):

```sh
btmux --profile work --port 8005
btmux --profile personal --port 8006
```

A profile's files are in `~/.local/state/btmux/<profile>/`. Only one server
can run a profile at a time.

---
title: "Configuration"
sidebar:
  order: 1
  label: "Overview"
---

btmux creates a configuration file on first launch:

```text
~/.config/btmux/config.toml
```

It respects `$XDG_CONFIG_HOME`, all fields are optional, and most changes are
picked up live. Log-level changes take effect after a restart. If the file
can't be parsed, btmux keeps the last valid configuration (the defaults at
startup) and shows a banner until the file is fixed; its Show error button opens
the full message. Run the following
command for the complete documented set of options:

```sh
btmux generate-config
```

## tmux behaviors

Kill confirmation, send-prefix, pane resizing, and `repeat-time` are described in
[btmux for tmux users](../start/tmux.md).

## macOS editing keys

On macOS, panes translate Cmd editing chords as Ghostty does: Cmd+Left/Right
send `C-a`/`C-e` and Cmd+Backspace `C-u`, even to programs using the kitty
keyboard protocol. Cmd+Delete sends `C-k`, which Ghostty doesn't bind.
Shift+Cmd+Left/Right send the arrow with shift and super (`CSI 1;10 D`), so
programs that support selection can select to the line edge.
Option+Left/Right and Option+Backspace move and delete by word without it. Set
`[terminal] macos-edit-keys = false` to leave Cmd chords to the browser.

## Editor

`editor` chooses what `<prefix> + e`, the file browser, and Git mode open; see
[Editors](../features/editors.md).

The `browser` action (`<prefix> + b`) toggles a browser over the active pane; see
[Browser](../features/browser.md).

`[multipass]` sets up the VM sandboxed sessions run in; see
[Sandboxed sessions](../features/sandbox.md).

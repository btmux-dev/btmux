---
title: "Key overlay"
sidebar:
  order: 14
---

The key overlay shows the keys you press on screen, for recording tutorials
and demos. It knows btmux's key bindings, so viewers see what a key did, not
just which key it was.

It is off by default and turned on per client: the keyboard button in the
status bar, `toggle-key-overlay` in the command palette, or `?keys` in the
URL (`http://localhost:8004/?keys`) for scripted recordings. A browser
remembers the setting.

## What it shows

| You press                  | The overlay shows                          |
| -------------------------- | ------------------------------------------ |
| `ls -la` and Enter         | `ls␣-la⏎`, as one run                      |
| `Ctrl+C` twice             | `⌃ C ×2`                                   |
| The prefix                 | `⌃ A prefix…` until the next key           |
| `<prefix> + c`             | `⌃ A c New window`                         |
| `<prefix>` and no key      | `⌃ A prefix, no key` once the prefix times out |
| A key nothing is bound to  | `⌃ A y not bound`                          |
| Repeated resize keys       | `⌃ A ⌃↑ ×3 Resize pane up`                 |

Typing pauses longer than 1.5 s start a new run. Each entry fades 2.5 s after
its last key; up to four are shown. Off macOS, modifiers are spelled out
(`Ctrl`, `Alt`).

Keys typed into the file browser, editors, the browser surface, and other
dialogs show as plain keys.

## Passwords

While a pane's program reads a password, the overlay shows "Password input
hidden" instead of the keys. btmux tells from the terminal's modes: `sudo`,
`ssh`, `passwd`, `read -s`, and similar prompts turn echo off and keep line
editing on. Key bindings still show.

Keys typed into a terminal appear once the server confirms the mode they were
typed in, which takes a round trip. Until then, and if it can't tell, the
overlay shows "Terminal input hidden" rather than risk showing a password: on
Windows servers, which can't read the mode, it shows that for all terminal
typing, while key bindings and keys typed elsewhere still show.

It can't see prompts that read keys one at a time, in raw mode:

- a password prompt on another machine, in an `ssh` or sandbox session,
- fish's `read --silent`, and
- password fields in editors and web pages.

Check a recording before sharing it if you typed a password there.

## Configuration

```toml
[key-overlay]
mode = "all"          # or "chords": key bindings and Ctrl, Alt, or Cmd chords only
position = "bottom"   # "bottom-left", "bottom-right", "top", "top-left", "top-right"
```

`chords` hides plain typing, which suits demos of btmux itself. Bind a key to
`toggle-key-overlay` under `[keys]` to toggle the overlay with the prefix.

## Internals

A capture listener on `window` sees each key before any surface does, and
`useKeybindings` notes what it did with it (the prefix, a binding, a repeat)
before the key is committed (`lib/keyCast.ts`). Terminal input waits for
the server: while the overlay is shown, each pane socket asks for
`input_mode` reports (`watch_input`). The server checks the terminal's modes
(canonical without echo) after each output chunk, before that output goes out,
and around each input frame; each report says whether any check since the last
one hid input, and how many input frames came before it.

---
title: "Replay and multiple viewers"
sidebar:
  order: 2
---

Terminal scrollback in the browser still follows `[terminal].scrollback`. The
backend retains a separate output journal capped at 8 MiB per pane and 128 MiB
across journals. Evicting history advances a VT100 screen checkpoint, preserving
the visible screen and standard input modes while releasing older history.
Replay includes ordered resize events and never clears history on a resize.
The cap covers retained journal data, not total process or browser memory.

The checkpoint covers standard VT100 text, attributes, cursor, alternate screen,
and supported input modes. Ghostty-specific graphics and unsupported terminal
extensions are preserved in recent raw output but are not guaranteed after that
output is compacted into a checkpoint. Full compatibility with every Ghostty
extension would require a matching server-side Ghostty state serializer.

The first connected interactive viewer controls the shared PTY size; other
viewers adopt it. On disconnect, ownership passes to the oldest remaining viewer.
Mirrors never own dimensions or send input. Window and pane selection remain
shared per session, while session navigation remains local to each browser.

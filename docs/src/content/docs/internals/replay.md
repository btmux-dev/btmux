---
title: "Replay and multiple clients"
sidebar:
  order: 2
---

When a client attaches to a pane, the server replays its output so the
client can draw the current screen and scrollback.

## The journal

Each pane's **journal** records raw output and resize events in order.
The client feeds them to its terminal emulator.

Journals are capped at 8 MiB per pane and 128 MiB in total. This is separate
from `[terminal] scrollback`, which limits how many lines each client keeps.

## The checkpoint

When the journal fills, the server folds its oldest output into a
**checkpoint** that preserves screen state such as text, colors, and cursor
position. Replay starts with the checkpoint, then applies the remaining journal.

The checkpoint understands standard terminal features: text, attributes, the
cursor, the alternate screen, and input modes. Newer extensions, such as
inline images, survive while they are still in the journal, but not once they
have been folded into a checkpoint.

## Several clients on one pane

- The first client to attach sets the pane's size. Others show that size
  instead of fitting their own window.
- Another client takes over the size by typing, pasting, or clicking in the
  pane. When the owner disconnects, the longest-attached client takes over.
- A client whose window is too small for that size shows the pane clipped,
  with a **Fit to this screen** button that takes over the size.
- Thumbnails, in the window grid and the agent view, only watch. They never
  set the size or send input.

Resizing never clears history: resizes are part of the journal, so a replay
walks through them in order.

Which window and pane are active is shared by all clients. Which session a
client shows is its own.

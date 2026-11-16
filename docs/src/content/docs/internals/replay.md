---
title: "Replay and multiple viewers"
sidebar:
  order: 2
---

A client can attach to a pane at any time: after a reload, from a second
machine, or hours after the program started. The server has to show it what is
already on screen. This page explains how, and what happens when several
clients look at the same pane.

## The journal

The server keeps each pane's recent output in a **journal**: the raw bytes the
program wrote, in order, along with every resize. When a client attaches, the
server replays the journal into the client's terminal emulator, which ends up
in the same state as everyone else's.

Journals are capped at 8 MiB per pane and 128 MiB in total. This is separate
from `[terminal] scrollback`, which limits how many lines each client keeps.

## The checkpoint

When a journal is full, its oldest output is dropped. Simply cutting it off
would lose whatever that output set up: the text still on screen, colors, the
cursor, whether a full-screen program is running.

So the server folds dropped output into a **checkpoint**: a snapshot of the
screen that the remaining journal continues from. A replay starts with the
checkpoint, then the journal.

The checkpoint understands standard terminal features: text, attributes, the
cursor, the alternate screen, and input modes. Newer extensions, such as
inline images, survive while they are still in the journal, but not once they
have been folded into a checkpoint.

## Several clients on one pane

A program has one terminal size, but clients have different window sizes.

- The first client to attach sets the pane's size. Others show that size
  instead of fitting their own window.
- Another client takes over the size by typing, pasting, or clicking in the
  pane. When the owner disconnects, the longest-attached client takes over.
- Thumbnails, in the window grid and the agent view, only watch. They never
  set the size or send input.

Resizing never clears history: resizes are part of the journal, so a replay
walks through them in order.

Which window and pane are active is shared by all clients. Which session a
client shows is its own.

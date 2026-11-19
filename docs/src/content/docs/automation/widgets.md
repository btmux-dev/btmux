---
title: "Widgets from the shell"
sidebar:
  order: 5
---

`btmux pick`, `confirm`, and `input` show a widget at the cursor of the pane
they run in, for scripts and shell key bindings.

Widgets stay open across window switches and reloads until a client answers
or the command ends. Prefix bindings still work. If several clients show a
widget, the first answer counts.

| Exit status | Meaning                                                      |
| ----------- | ------------------------------------------------------------ |
| 0           | Answered                                                     |
| 1           | No, or nothing to choose from                                |
| 2           | Failed: run outside btmux, or no client showed the pane within 3 s |
| 130         | Canceled with Esc or `Ctrl+C`                                |

## Menus

`btmux pick` shows a menu and prints the choice:

```sh
git switch "$(git branch --format='%(refname:short)' | btmux pick --prompt Branch)"
```

Each line of stdin supplies an item. Text after a tab is a searchable
description and is omitted from the output. Type to filter (each
space-separated term ignores case unless it has an uppercase letter, as in
fzf), move with Up, Down, Tab, `Ctrl+N`, or `Ctrl+P`, and select with Enter or
a click.

| Flag                | Effect                                                                                  |
| ------------------- | --------------------------------------------------------------------------------------- |
| `--prompt TEXT`     | Placeholder for the filter                                                              |
| `-q`, `--query TEXT` | The filter's initial text                                                              |
| `-m`, `--multi`     | Tab and Shift-Tab mark and move, and a click marks; Enter prints the marked items, in input order, or the current one |
| `--no-sort`         | Keep input order while filtering, rather than ranking label prefixes first              |
| `--read0`           | Items end in NUL and are taken whole, so they can hold line breaks and tabs, shown as `␤` and `␉`, with no descriptions |
| `--print0`          | Printed items end in NUL                                                                |
| `-1`, `--select-1`  | Print the item without a menu if only one matches the query                             |
| `-0`, `--exit-0`    | Exit 1 without a menu if nothing matches the query                                      |

Status 2 lets a key binding fall back to the terminal, as the
[fish integration](../features/fish.md)'s Tab completion does.

## fzf

`btmux fzf` shows a btmux menu for supported invocations and falls back to
fzf for the rest.

```sh
git switch "$(git branch --format='%(refname:short)' | btmux fzf --height=40% --query main)"
```

- **Understood:** `--query`, `--prompt`, `--multi`, `--no-sort`, `--read0`,
  `--print0`, `--select-1`, `--exit-0`, and `--tac`. As in fzf, marked items
  print in the order marked.
- **Ignored:** appearance, matching, and preview options, and `--bind`s that
  only rename movement, marking, accepting, or canceling. The menu has no
  preview and matches every space-separated term.
- **Handed to the real fzf:** anything else, such as `--expect`,
  `--print-query`, `--with-nth`, `--header`, or a `--bind` with `reload`,
  `execute`, or an event. So are a query with `--select-1` or `--exit-0`
  (fzf's matching decides those), no piped stdin, more than 16 MiB of input,
  a run outside btmux, and a pane no client can show the menu on.

`FZF_DEFAULT_OPTS` and `FZF_DEFAULT_OPTS_FILE` apply as in fzf. Unlike fzf,
the menu waits for stdin to end before it opens, so a producer that never
finishes needs the real one.

To use btmux for programs that call `fzf` by name, link it as `fzf` before
the original on `PATH`. The wrapper finds the original later on `PATH`.

```sh
ln -s "$(command -v btmux)" ~/.local/bin/fzf
```

This doesn't reach programs built with an absolute path to fzf, such as zoxide from
Nixpkgs.

## Questions

`btmux confirm` asks a yes/no question, exiting 0 for yes and 1 for no:

```sh
btmux confirm "Delete $file?" && rm -- "$file"
```

`y` and `n` answer at once. Left, Right, and Tab move between the buttons, and
Enter answers with the selected one, which starts as No unless you pass
`--default-yes`. `&&` runs the command only on a yes answer.

## Text

`btmux input` asks for a line of text and prints it, exiting 0 even if it is empty:

```sh
name="$(btmux input --prompt "Branch name" --default "feat/")" && git switch -c "$name"
```

It is shown as typed, so use `read -s` for passwords.

## API

The commands wrap `POST /api/panes/<pane-id>/widget`, which takes up to 16 MiB:

| `type`    | Fields                                                                         | `answer` when answered             |
| --------- | ------------------------------------------------------------------------------ | ---------------------------------- |
| `pick`    | `items` (each a `label` and an optional `description`), optional `prompt`, `query`, `multi`, and `no_sort` | `{"type": "pick", "indices": [n]}` |
| `confirm` | `message`, `default_yes`                                                       | `{"type": "confirm", "yes": bool}` |
| `input`   | optional `prompt` and `default`                                                | `{"type": "input", "text": "..."}` |

It replies with `{"answer": ...}` once the user answers (`null` if canceled),
or with 409 if no client shows the pane or the pane already has a widget open.

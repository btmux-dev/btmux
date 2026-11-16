---
title: "Widgets from the shell"
sidebar:
  order: 5
---

`btmux pick`, `confirm`, and `input` show a widget at the cursor of the pane they
run in, for scripts and shell key bindings. Prefix bindings still work, so you can
leave and come back: a widget stays open until it is answered or the command is
killed, including across tab reloads. With several tabs on the pane, each shows it
and the first answer counts. Status 2 always means it failed: it ran outside
btmux, or no tab showed the pane within 3 s. Status 130 means it was canceled with
Esc or `Ctrl+C`.

## Menus

`btmux pick` shows a menu and prints the choice:

```sh
git switch "$(git branch --format='%(refname:short)' | btmux pick --prompt Branch)"
```

Items are stdin's lines. A tab starts a description, which is shown and
filtered on but not printed. Typing filters; Up, Down, Tab, `Ctrl+N`, and
`Ctrl+P` move; Enter or a click chooses. It exits 0 once an item is chosen and
printed, and 1 if stdin had no items.

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

`btmux fzf` takes fzf's arguments and shows the same menu when it can do what they
ask:

```sh
git switch "$(git branch --format='%(refname:short)' | btmux fzf --height=40% --query main)"
```

It understands `--query`, `--prompt`, `--multi`, `--no-sort`, `--read0`, `--print0`,
`--select-1`, `--exit-0`, and `--tac`. As in fzf, marked items print in the order marked. It drops appearance, matching, and preview
options (the menu always shows no preview and matches every space-separated term), and
`--bind`s of keys that only name movement, marking, accepting, or canceling. `FZF_DEFAULT_OPTS` and
`FZF_DEFAULT_OPTS_FILE` apply as they do in fzf.

Everything else runs the real fzf, unchanged: other arguments such as `--expect`,
`--print-query`, `--with-nth`, `--header`, or `--bind` with `reload`, `execute`, or an
event such as `load`; a query with `--select-1` or `--exit-0`, since fzf's matching decides those; no piped
stdin; outside a btmux pane; more than 16 MiB of input; or when no tab can show the menu. Unlike fzf, the menu waits for stdin to end before it opens, so a producer
that never finishes needs the real one.

For programs that run `fzf` themselves, link btmux under that name, ahead of the real
fzf on your `PATH`. It finds the real one further along, so this is safe outside
btmux too:

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
`--default-yes`. Since any failure is nonzero too, `&&` skips the command then as
well.

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

It replies when the user answers with `{"answer": ...}` (`null` if canceled), or
409 if no tab shows the pane or it already has a widget open.

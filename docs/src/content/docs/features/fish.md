---
title: "Fish integration"
sidebar:
  order: 13
---

The fish integration makes fish's colors follow the btmux color scheme, and
can replace fish's completion and history menus with btmux's own.

```sh
btmux install-fish-integration   # the theme alone: btmux generate-fish-theme > btmux.theme
```

This writes `~/.config/fish/themes/btmux.theme` and `conf.d/btmux.fish` (respecting
`$XDG_CONFIG_HOME`). The theme uses named ANSI colors following base16 roles, so
it tracks the btmux scheme, including live changes. The snippet selects it only
inside btmux panes and overrides universal `fish_color_*` variables there. The
Home Manager module installs both files when `programs.fish.enable` is set.

To use [`btmux pick` menus](../automation/widgets.md#menus) in place of fish's own for Tab
completion, `Ctrl+R` history search, and `cdh`, add this to `config.fish` (with
Home Manager, set `programs.btmux.fishPickMenus = true` instead):

```fish
set -g btmux_pick_menus 1
```

It replaces any `Tab` or `Ctrl+R` binding and `cdh` function of your own in btmux
panes, and is read at the first prompt. fish's own behavior remains as a fallback
when no tab can show a menu. Tab still completes directly with at most one
candidate, and normal mode in vi and hybrid bindings keeps `Ctrl+R` as redo.

`Ctrl+R` lists commands newest first, filtered on the current line. Multi-line
commands show `␤` for their line breaks and tabs as `␉`.

`complete -C` doesn't say how fish would quote a candidate or whether to add a
space after it, so a choice from the menu has its spaces escaped and gets no
trailing space. Other special characters, such as parentheses, need editing by
hand. Candidates are computed before the menu opens, so slow completions delay it
as they delay fish's own.

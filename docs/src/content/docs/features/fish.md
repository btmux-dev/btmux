---
title: "Fish integration"
sidebar:
  order: 13
---

The fish integration makes fish's colors follow the btmux color scheme, and
can replace fish's completion and history menus with btmux's own.

```sh
btmux install-fish-integration
```

This writes two files under `~/.config/fish` (or `$XDG_CONFIG_HOME/fish`):

- `themes/btmux.theme` uses named ANSI colors by their base16 roles, so it
  follows the btmux scheme, live changes included.
- `conf.d/btmux.fish` selects that theme inside btmux panes only, and
  overrides universal `fish_color_*` variables there.

`btmux generate-fish-theme` prints the theme alone. The Home Manager module
installs both files when `programs.fish.enable` is set.

## Menus

To use [`btmux pick` menus](../automation/widgets.md#menus) for Tab
completion, `Ctrl+R` history search, and `cdh`, add this to `config.fish`:

```fish
set -g btmux_pick_menus 1
```

With Home Manager, set `programs.btmux.fishPickMenus = true` instead.

- In btmux panes this replaces your own `Tab` and `Ctrl+R` bindings and `cdh`
  function. It is read at the first prompt.
- fish's own menus remain the fallback when no client can show one.
- Tab still completes directly when there is at most one candidate.
- In vi and hybrid bindings, normal mode keeps `Ctrl+R` as redo.
- `Ctrl+R` lists commands newest first, filtered on the current line. Line
  breaks in a command show as `␤` and tabs as `␉`.

Two limits come from fish. It doesn't say how it would quote a candidate or
whether a space follows it, so a chosen candidate gets its spaces escaped and
no trailing space; other special characters, such as parentheses, need editing
by hand. And candidates are computed before the menu opens, so a slow
completion delays it, as it delays fish's own.

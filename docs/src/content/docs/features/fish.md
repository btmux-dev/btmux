---
title: "Fish integration"
sidebar:
  order: 13
---

Match fish's colors to the btmux color scheme and optionally use btmux menus
for completion and history.

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

A selected completion escapes spaces and omits a trailing space. Escape
other special characters, such as parentheses, by hand. The menu opens after
fish finishes computing candidates.

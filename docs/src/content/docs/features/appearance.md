---
title: "Appearance"
sidebar:
  order: 10
---

Preview colors, fonts, and wallpapers in
[Settings](../configuration/index.md#settings) before saving.

## Dark and light

`appearance` selects the dark or light color scheme.

```toml
appearance = "auto"   # or "dark", "light"
colors-dark = "kauz-dark"
colors-light = "kauz-light"
```

`"auto"` follows the system setting of the client that connected or was
focused last. Clients and programs in panes see the same scheme. The sidebar's appearance
button and `choose-appearance` switch it until the next restart.

## Color schemes

`colors-dark` and `colors-light` take base16 or base24 schemes, given as:

- a bundled scheme: `btmux-default-dark`, `btmux-default-light` (the
  defaults), `kauz-dark`, or `kauz-light`;
- the name of a YAML file in `~/.config/btmux/colors/`, without its `.yaml` or
  `.yml` extension. It wins over a bundled scheme of the same name;
- an absolute or `~/`-relative path to a YAML file; or
- an `http://` or `https://` URL to one, fetched whenever the configuration
  loads.

Or define a palette inline, which overrides the matching option:

```toml
[theme-dark]
base00 = "#1e1e2e"
# ... through base0F, and optionally base10 to base17
```

A palette file may hold its colors at the top level or under `palette`. It
must contain `base00`–`base0F`; a file without them is skipped with a warning
in the server log. It counts as base24 only with all of `base10`–`base17`.

The command palette's colors picker lists the bundled schemes and those in the
colors directory, and sets the scheme of the mode currently shown.

## Fonts

```toml
[terminal]
font-family = "JetBrains Mono"
font-size = 16
font-weight = 400
```

These set the terminal, the Neovim GUI, and code-like UI text such as paths,
session and window names, and key hints. Other UI text uses Geist Sans.
Bundled families are Geist Mono (the default), JetBrains Mono, Fira Code,
Cascadia Code, Source Code Pro, and Departure Mono.

## Wallpapers

Wallpapers are off by default. Set `wallpaper` to an image URL or path, add
[shader layers](shaders.md), or both: the image sits beneath the shaders.

```toml
wallpaper = "~/Pictures/bg.png"
wallpaper-opacity = 0.1
wallpaper-blur = 0
wallpaper-saturate = 0.05
```

The server fetches remote images itself and serves them to the browser.

## Pane titles and effects

- `show-pane-titles = true` adds a title bar to each pane with its title,
  directory, and size.
- `pane-switch-border` draws a border on the pane you switch to: `"wipe"` (the
  default), `"trace"`, `"sweep"`, or `"none"`.
- `backdrop-blur` and `backdrop-dim` set the backdrop behind dialogs.
- `popup-opacity` and `popup-blur` set the editor's popups (completions,
  hovers, pickers): how opaque their background is and how much they blur
  what's behind them.
- `toolbar-opacity`, `toolbar-blur`, and `toolbar-saturate` set the side bar
  and status bar: how opaque their background is, and how much they blur and
  saturate what shows through. By default they're clear, with a 40 px blur.
- Panes grow and shrink into place when they split, close, or change layout,
  while the programs in them resize once, at the start.
- `animations = false` turns off all animated effects, as does the system's
  reduced motion setting for panes.

## For programs in panes

Programs can ask for the scheme: btmux answers OSC 4/10/11/12 color queries
and `CSI ? 996 n`, and sends `CSI ? 997 ; 1|2 n` to programs that enabled mode
2031 whenever the theme changes, including a switch between dark and light.
Fish uses this to pick its light or dark theme variant; see the
[fish integration](fish.md).

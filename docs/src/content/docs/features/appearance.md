---
title: "Appearance"
sidebar:
  order: 1
---

Press `<prefix> + :` and choose `config: open settings` to open the
browser-based editor. Applying a change creates a process-local preview; the
editor never writes `config.toml`. Preview overrides disappear when btmux
restarts or the config file reloads. Use the editor's generated TOML and copy
action to persist your choices.

Configuration includes terminal options, session and window sort order, pane
title bars, vi-style navigation, bundled font families and weights, image or
procedural wallpapers, and steady-state, session-switch, and pane-switch WebGL
effects.

The `[terminal]` `font-family` and `font-weight` set the terminal, the Neovim
GUI, and code-like UI text such as paths, session and window names, and key
hints. Other UI text uses Geist Sans.

Wallpapers are disabled by default. Set `wallpaper` to an image URL or path,
add `[[wallpaper-shader]]` layers to stack shaders and effects, or both: the image
is the bottom layer, beneath the shaders (see [Shader wallpapers](shaders.md)).
The server fetches remote images itself and serves them to the browser.

btmux checks GitHub for newer releases about once a day and shows a toast when
one is available; About btmux also checks when it opens. It never downloads or installs anything; upgrade with whatever
method you installed with. Set `check-for-updates = false` to turn the check off.

btmux has a dark and a light color scheme. `appearance` picks which one is
shown: `"auto"` (the default) follows the system setting of the browser that
connected or was focused last, while `"dark"` and `"light"` pin one. The same
scheme is shown in every tab and reported to programs in panes. The sidebar's
appearance button and the `choose-appearance` command switch it for the
session.

```toml
appearance = "auto"
colors-dark = "kauz-dark"
colors-light = "kauz-light"
```

`colors-dark` (default `btmux-default-dark`) and `colors-light` (default
`btmux-default-light`) accept:

- the name of a bundled scheme: `btmux-default-dark`, `btmux-default-light`,
  `kauz-dark`, or `kauz-light`;
- the name of a base16/base24 YAML file in `$XDG_CONFIG_HOME/btmux/colors/`
  (falling back to `~/.config/btmux/colors/`), which takes precedence over a
  bundled scheme of the same name. The name is the filename without its
  `.yaml` or `.yml` extension; files with other extensions are ignored;
- an absolute or `~/`-relative local YAML path; or
- an `http://` or `https://` URL to a YAML palette.

An inline `[theme-dark]` or `[theme-light]` table of `base00`–`base0F` (and
optionally `base10`–`base17`) overrides the matching option.

Palettes may be defined at the top level or nested under `palette`. They must
contain `base00`–`base0F`; a file without them is skipped with a warning in the
server log. A palette is base24 only if it also has all of `base10`–`base17`;
with any of them missing it is treated as base16. Remote palettes are fetched
whenever the configuration loads.

Schemes in the colors directory and the bundled schemes appear in the colors
picker of the command palette, which sets the scheme of the mode currently
shown, and in Settings. Changes there are session-only; set `appearance`,
`colors-dark`, or `colors-light` in `config.toml` to keep them.

Programs in a pane can query the scheme: btmux answers OSC 4/10/11/12 color
queries and `CSI ? 996 n`, and sends `CSI ? 997 ; 1|2 n` to programs that
enabled mode 2031 whenever the theme changes, including when `appearance`
switches between dark and light. Fish uses this to set
`fish_terminal_color_theme` and switch light/dark theme variants.

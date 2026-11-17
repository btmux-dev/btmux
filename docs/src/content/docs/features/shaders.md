---
title: "Shader wallpapers"
sidebar:
  order: 11
---

A shader wallpaper is a stack of up to 8 layers drawn behind your panes.
**Generators** draw something new: gradients, noise, `aurora`, `plasma`,
`smoke`, `boids`. **Effects** process every layer beneath them: `pixelate`,
`film-grain`, `crt-screen`, `blur`, `kaleidoscope`. Some of each react to the
pointer or the terminal cursor.

The shaders are the MIT-licensed [Shaders](https://shaders.com/docs/guide)
components, bundled with btmux; nothing is downloaded at runtime.

## Building a stack

Use Settings → Wallpaper. It lists every shader and blend mode, and lets you
add, reorder, disable, and remove layers and edit each layer's parameters,
with a live preview. **Randomize parameters** below a layer's controls rolls a
coordinated set for that layer. **Save** writes the stack to `config.toml`.

In TOML, layers are listed bottom first:

```toml
wallpaper-opacity = 0.25
wallpaper-speed = 0.2
wallpaper-fps = 30
wallpaper-resolution = 0.4

[[wallpaper-shader]]
shader = "ink-flow"
[wallpaper-shader.params]
color-mode = "custom"
radius = 0.25

[[wallpaper-shader]]
shader = "pixelate"
params = { scale = 40 }

[[wallpaper-shader]]
shader = "film-grain"
blend-mode = "soft-light"  # how the layer composites over those beneath it
opacity = 0.6              # 0.0-1.0, default 1.0
enabled = true             # keep a layer configured but hidden
interactive = true         # whether pointer and terminal-cursor input reach it
```

Parameter names are kebab-case, as Settings exports them. A layer without
`params` uses its shader's defaults.

## With an image

A [`wallpaper`](appearance.md#wallpapers) image is drawn beneath the stack, so
effects process it and blend modes composite over it. It then renders at
`wallpaper-resolution`; raise that for a sharp photo.

## Speed and cursor

- `wallpaper-speed` multiplies each shader's own speed. `0` freezes it.
- `wallpaper-fps` (10–60) and `wallpaper-resolution` (0.1–1.0) trade quality
  for GPU time.
- `wallpaper-shader-follows-mouse-cursor` and
  `wallpaper-shader-follows-keyboard-input` choose whether the pointer and the
  terminal cursor move interactive layers.

Rendering pauses in background tabs and with `animations = false`.

## Requirements

Shaders need WebGPU. In a browser without it, or after a GPU failure, the
wallpaper image shows instead, or else the theme background. WebGPU also needs
a secure origin: `localhost`, or [HTTPS](remote-access.mdx).

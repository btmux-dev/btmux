---
title: "Browser"
sidebar:
  order: 6
---

`<prefix> + b` opens a browser [over the active pane](editors.md#over-a-pane),
for checking a dev server or reading docs beside your terminal. On touch
screens, the key bar has a button to hide it.

On macOS and Linux, btmux runs headless Chrome or Chromium from the server's
`PATH` or `/Applications`. Use `localhost` to reach the server. All clients
share one page. New-tab links open there, and btmux answers JavaScript
dialogs automatically.

The browser has its own profile for logins and reopens its last page after
a restart.

## Ads and autoplay

The browser blocks known ad servers and keeps video and audio from playing
until you click or press a key. Ads are blocked by host name, so ads a site
serves itself still show, and blocked ones leave their empty space.

Changing either setting restarts the browser and reopens the page.

```toml
[browser]
block-ads = false
block-autoplay = false
```

## Copying

To copy, select text and press Cmd+C (Ctrl+C outside macOS). A page's own
copy buttons don't reach your clipboard.

## Address bar

Address bar searches use DuckDuckGo by default. In `search-url`, `%s` stands
for the search text.

```toml
[browser]
search-url = "https://www.google.com/search?q=%s"
```

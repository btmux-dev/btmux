---
title: "Browser"
sidebar:
  order: 6
---

`<prefix> + b` opens a browser [over the active pane](editors.md#over-a-pane),
for checking a dev server or reading docs beside your terminal. On touch
screens, the key bar has a button to hide it.

It runs a headless Chrome or Chromium on the server, found on the server's
`PATH` or in `/Applications`, so it shows any site, and `localhost` means the
server. There is one browser with one page per btmux server, so every client
sees the same page, and links that would open a new tab open in that page.
JavaScript dialogs are answered automatically.

Requires macOS or Linux, and Chrome or Chromium installed on the server.

Its profile, with your logins, is kept apart from your everyday browser's. It
reopens your last page after a restart.

## Ads and autoplay

The browser blocks known ad servers and keeps video and audio from playing
until you click or press a key. Ads are blocked by host name, so ads a site
serves itself still show, and blocked ones leave their empty space.

Either can be turned off. Changing them restarts the browser and reopens your
page:

```toml
[browser]
block-ads = false
block-autoplay = false
```

## Copying

To copy, select text and press Cmd+C (Ctrl+C outside macOS). A page's own
copy buttons don't reach your clipboard.

## Address bar

Text in the address bar that isn't a URL is searched with DuckDuckGo, or with
`search-url`, where `%s` is the text:

```toml
[browser]
search-url = "https://www.google.com/search?q=%s"
```

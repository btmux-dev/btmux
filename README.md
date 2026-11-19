# btmux

[![Release](https://img.shields.io/github/v/release/btmux-dev/btmux)](https://github.com/btmux-dev/btmux/releases/latest)
[![CI](https://github.com/btmux-dev/btmux/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/btmux-dev/btmux/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/github/license/btmux-dev/btmux)](LICENSE)

A persistent terminal workspace and session manager for desktop, browser, and
mobile. tmux-style UI, built-in dev tools, and coding agent support. Runs on
macOS, Linux, and Windows. Powered by
[wterm](https://github.com/vercel-labs/wterm),
[libghostty](https://github.com/ghostty-org/ghostty), and
[Tauri](https://github.com/tauri-apps/tauri).

**[Documentation](https://btmux-dev.github.io/btmux/)** ·
[Installation](https://btmux-dev.github.io/btmux/start/installation/) ·
[First steps](https://btmux-dev.github.io/btmux/start/first-steps/) ·
[Changelog](https://btmux-dev.github.io/btmux/project/changelog/)

## Highlights

- tmux-style sessions, windows, splits, zoom, and key bindings
- Server-side sessions with simultaneous clients
- Tailscale HTTPS access with QR sign-in for mobile
- File and Git browser, process viewer, and web browser
- Native editor with language servers, Neovim GUI, and VS Code
- Coding agent status tracking, usage, and notifications
- Sandboxed sessions in a Multipass VM with selected directories mounted
- LaTeX overlay, shader wallpapers, and base16/base24 themes
- tmux-style CLI, REST API, MCP server, and agent skill
- Single self-contained binary (~20 MB), and a Nix Home Manager module

## Install

**Desktop app** on macOS:

```sh
brew install --cask btmux-dev/tap/btmux
```

On Linux and Windows, get the `.deb` or the setup `.exe` from
[Releases](https://github.com/btmux-dev/btmux/releases/latest). Windows support is
experimental.

**Server only**, to use btmux in a browser, on macOS or Linux:

```sh
curl -fsSL https://raw.githubusercontent.com/btmux-dev/btmux/main/scripts/install.sh | bash
# or: brew install btmux-dev/tap/btmux
```

[Installation](https://btmux-dev.github.io/btmux/start/installation/) covers
Windows, running as a service, upgrading, Nix, and building from source.

## Quick start

Open the **desktop app** to start its own server or choose an existing one.

Run `btmux` to start the **standalone server**. It opens
`http://127.0.0.1:8004` and prints the path to its token file. Sign in as
`btmux` with the token as your password.

Key bindings start with a prefix, as in tmux, by default `Ctrl+A`. Press
`<prefix> + ?` to see them all, or `<prefix> + :` to open the command palette.
Coming from tmux? See
[btmux for tmux users](https://btmux-dev.github.io/btmux/start/tmux/).

## Contributing

[Development](https://btmux-dev.github.io/btmux/project/development/) covers
building and running btmux from a checkout.

## License

[MIT](LICENSE)

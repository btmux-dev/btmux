# btmux

[![Release](https://img.shields.io/github/v/release/btmux-dev/btmux)](https://github.com/btmux-dev/btmux/releases/latest)
[![CI](https://github.com/btmux-dev/btmux/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/btmux-dev/btmux/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/github/license/btmux-dev/btmux)](LICENSE)

A terminal emulator and tmux-style session manager. Runs as a desktop app and
in the browser (including on mobile), on macOS, Linux, and Windows. Powered by
[wterm](https://github.com/vercel-labs/wterm),
[libghostty](https://github.com/ghostty-org/ghostty), and
[Tauri](https://github.com/tauri-apps/tauri).

**[Documentation](https://btmux-dev.github.io/btmux/)** ·
[Installation](https://btmux-dev.github.io/btmux/start/installation/) ·
[First steps](https://btmux-dev.github.io/btmux/start/first-steps/) ·
[Changelog](https://btmux-dev.github.io/btmux/project/changelog/)

## Highlights

- Familiar tmux concepts and key bindings: sessions, windows, splits, zoom, and more
- Server-side sessions: connect from any number of clients
- Tailscale integration: serve over HTTPS on your tailnet; sign in from a phone or tablet by scanning a QR code
- Batteries included: file browser, Git GUI, process viewer, browser, an editor with language servers, Neovim GUI, VS Code integration
- Coding agent status tracking, usage, and notifications
- Sandboxed sessions in a Multipass VM that sees only the directories you mount
- LaTeX rendering overlay
- Interactive, composable shader wallpapers
- base16/base24 color schemes, dark and light mode
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

**Desktop app**: open btmux. It starts its own server and connects, with no
sign-in.

**Server**: run `btmux`. It opens `http://127.0.0.1:8004` and prints the path
of its access token file. Sign in with the username `btmux` and the file's
contents as the password.

Key bindings start with a prefix, as in tmux, by default `Ctrl+A`. Press
`<prefix> + ?` to see them all, or `<prefix> + :` to open the command palette.
Coming from tmux? See
[btmux for tmux users](https://btmux-dev.github.io/btmux/start/tmux/).

## Contributing

[Development](https://btmux-dev.github.io/btmux/project/development/) covers
building and running btmux from a checkout.

## License

[MIT](LICENSE)

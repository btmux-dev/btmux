---
title: "Development"
sidebar:
  order: 2
---

Install [Rust](https://rustup.rs), [Bun](https://bun.sh), and
[`just`](https://github.com/casey/just).

```sh
git clone https://github.com/buntec/btmux.git
cd btmux
just setup  # install dependencies
just dev    # backend on :8044, frontend on http://localhost:5173
just check  # type-check Rust and TypeScript
```

Run `just` to list all recipes. The dev server uses the `dev` profile and the
fixed access token `1234`, which the frontend at `http://localhost:5173` asks
for in a form.

To install your build, run `cargo install --path .` after `just setup`. Install
a [service](../start/installation.mdx#run-it-as-a-service) from that stable
path, not from `target/`, where `cargo clean` or a rebuild would break it.

## Platform notes

- **macOS:** install the Xcode Command Line Tools (`xcode-select --install`).
  Listing sockets generates bindings from the macOS SDK with libclang; if
  libclang is installed separately, set `LIBCLANG_PATH` to its library
  directory.
- **Windows:** install the MSVC build tools Rust needs. `just setup` and the
  desktop recipes run in PowerShell; the others need `sh`, so run them from Git
  Bash or put `C:\Program Files\Git\bin` on `PATH`.

The desktop app's build and release are covered in its
[README](../../../../../desktop/README.md).

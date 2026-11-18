# btmux desktop

See the [desktop app guide](../docs/src/content/docs/start/desktop.md) for
connecting to servers, keeping shells running, and installing the CLI.

## Build and test

| Command | Action |
| --- | --- |
| `just desktop-dev` | Build the frontend and server, then open the Tauri app |
| `just desktop-build` | Create a bundle under `desktop/src-tauri/target/release/bundle/` |
| `just desktop-check` | Compile the launcher and bundled server |
| `cargo test --manifest-path desktop/src-tauri/Cargo.toml` | Test registration privacy and authenticated discovery |
| `just test-desktop-launcher` | Test the startup picker in Playwright without starting a server |

The macOS bundle is `macos/btmux.app`. To also build a disk image, run
`cd desktop && bun run build -- --bundles dmg` after building the sidecar.
Windows bundles an installer under `nsis/`. On Windows, `just setup` and the
desktop recipes use Windows PowerShell. The app installs to
`%LOCALAPPDATA%\btmux Desktop`, separate from the server's state directory.

After changing `app-icon.svg`, run
`cd desktop && bunx tauri icon app-icon.svg` to regenerate the icons.

## Server discovery

Standalone servers write an owner-only `state.server` beside the profile's
`state.json`. It contains the local endpoint and credential, including a
credential set through `BTMUX_AUTH_TOKEN`. Normal shutdown removes the file.
The launcher checks entries through the authenticated API and skips stale or
inaccessible entries.

The app and server must share a state directory. Wildcard listeners are
reached through loopback. Discovery skips listeners bound only to a
non-loopback address. It can also find servers from v0.0.106 onward running
the default profile on port 8004 through `state.token`. Restart older
named-profile or custom-port servers with the updated binary to register them.

The app's server uses the `desktop` profile on an available loopback port. It
registers only when started with **Keep it running after btmux quits**. In
that mode it starts detached and uses the profile's saved credential.
**Share it on my tailnet** saves its port in `tailnet-port` in the app's
settings folder, below the OS's ephemeral port range. If the port is taken,
start without sharing or delete the file to pick a new port.
**Upgrade** posts the bundled binary to `/api/server/upgrade`, as
`btmux upgrade` does, after rechecking that the server is older, and waits
for a new instance with the same pid (`src/upgrade_status.rs`, shared with the
server).

The app validates remote credentials through `/api/info` and stores them in
an owner-only `remote-servers.json` in the app's settings folder
(`~/Library/Application Support/com.btmux.desktop` on macOS). The picker checks
remote servers in the background. Rust installs the credentials as HttpOnly
cookies before opening the server UI.

On Linux and Windows, release builds open a new window in the running app
when launched again. Development builds run separately.

## Command-line tool

The Windows installer puts a hard link to the bundled server at
`bin\btmux.exe` and adds that directory to the end of the user's `PATH`,
keeping an existing entry in place. Install and uninstall wait for running
copies from the install folder to exit.

On macOS, **Install Command-Line Tool…** asks before replacing an existing
entry at `/usr/local/bin/btmux` unless it is a stale symlink. The menu also
warns if another binary comes first on the shell's `PATH`. Nix packages omit
the menu item because each update moves the app to a new path.

## Release builds

Bundle a matching server binary for each target architecture. Release CI
uploads a zipped macOS app, Linux `.deb` packages, and Windows NSIS installers
(`-setup.exe`, x64 and ARM64). A Windows build failure leaves the other
artifacts available.

CI signs the macOS app and server with a Developer ID, notarizes both, and
staples the app. It uses `scripts/macos-sign.sh` for the server and Tauri's
`APPLE_*` variables for the app. See `release.yml` for the secrets.
The app uses Tauri's `macos-private-api` feature for transparency, which
precludes Mac App Store distribution. Windows installers are unsigned.

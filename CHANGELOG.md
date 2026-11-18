# Changelog

Notable changes in each btmux release. Releases before 0.0.197 are listed on
[GitHub](https://github.com/btmux-dev/btmux/releases).

There are no compatibility guarantees before 0.1.0: config fields, key
actions, saved state, and API types can change in any release.

## [Unreleased]

### Changed

- Language servers' output, starts, and exits go to `lsp.log.YYYY-MM-DD`
  beside btmux's own log (the last 14 are kept), rather than to `btmux.log`
  at the `debug` level only.
  ([#168](https://github.com/btmux-dev/btmux/pull/168))

## [0.0.210] - 2026-11-18

### Added

- The desktop app upgrades a desktop server kept running after it quits:
  when the app is newer, the server's card in the picker shows **Upgrade**,
  which moves it to the app's version in place, keeping its shells when the
  versions are compatible.
  ([#166](https://github.com/btmux-dev/btmux/pull/166))

### Changed

- The Homebrew tap moved to `btmux-dev/tap`: `brew install btmux-dev/tap/btmux`.
  Existing `buntec/btmux` taps keep updating.
  ([#164](https://github.com/btmux-dev/btmux/pull/164))
- The status bar shows a button for the configured editor only, not one for
  each editor the server can run; the palette's `editor-*` actions still
  toggle the others.
  ([#165](https://github.com/btmux-dev/btmux/pull/165))

### Fixed

- Markdown previews show images in a paragraph, such as badges, on one line,
  as GitHub does, rather than one per line.
  ([#164](https://github.com/btmux-dev/btmux/pull/164))

## [0.0.209] - 2026-11-18

### Added

- A key overlay that shows pressed keys and the key bindings they ran, for
  recording demos. It is off by default; the status bar's keyboard button
  turns it on. Keys typed at a password prompt are hidden.
  ([#160](https://github.com/btmux-dev/btmux/pull/160))
- Justfile support in the native editor: syntax highlighting, with recipe
  bodies highlighted as shell, and just-lsp as a built-in language server.
  ([#161](https://github.com/btmux-dev/btmux/pull/161))

### Changed

- A `[[language]]` table's `file-types` are extensions only; whole file names,
  such as `Makefile`, go in `file-names` and match ignoring case.
  ([#161](https://github.com/btmux-dev/btmux/pull/161))
- The agent view shows agent logos in their brand colors instead of gray.
  ([#163](https://github.com/btmux-dev/btmux/pull/163))

## [0.0.208] - 2026-11-18

### Added

- A documentation site with a desktop walkthrough and references for commands
  and configuration. ([#135](https://github.com/btmux-dev/btmux/pull/135))
- The desktop app puts its bundled `btmux` command on `PATH`: on macOS through
  **btmux → Install Command-Line Tool…**, on Windows from the installer.
  ([#152](https://github.com/btmux-dev/btmux/pull/152))
- Nix syntax highlighting in the native editor.
  ([#151](https://github.com/btmux-dev/btmux/pull/151))

### Changed

- The agent view groups agents by state, most urgent first, and shows what
  each is asking, how long it has waited, and its branch. Clicking an agent
  goes to its pane; live tiles are shown only through the eye button.
  ([#155](https://github.com/btmux-dev/btmux/pull/155))

### Fixed

- `<prefix> + [` opens the pane's capture in the `editor` setting's editor
  over the pane, or `$EDITOR` in a popup with `editor = "pane"`, rather than
  typing a command into the pane, so it works in agent panes and other
  programs. ([#158](https://github.com/btmux-dev/btmux/pull/158))
- GitHub status finds pull requests for branches pushed to a URL and for
  renamed fork checkouts. ([#149](https://github.com/btmux-dev/btmux/pull/149))

## [0.0.207] - 2026-11-17

### Added

- **A native editor**, now the default for `<prefix> + e`: shared live between
  clients, with language servers, quick open, Vim mode, and Git signs.
  ([#134](https://github.com/btmux-dev/btmux/pull/134), [#137](https://github.com/btmux-dev/btmux/pull/137), [#138](https://github.com/btmux-dev/btmux/pull/138), [#143](https://github.com/btmux-dev/btmux/pull/143),
  [#145](https://github.com/btmux-dev/btmux/pull/145))
- `btmux upgrade` keeps the native editor's unsaved buffers.
  ([#136](https://github.com/btmux-dev/btmux/pull/136))
- A **Fit to this screen** button on a pane that another client sized larger.
  ([#147](https://github.com/btmux-dev/btmux/pull/147))
- Links in notification toasts open. ([#146](https://github.com/btmux-dev/btmux/pull/146))

### Changed

- `editor` defaults to `native` instead of `neovim`.

### Fixed

- Dictation on iOS. ([#148](https://github.com/btmux-dev/btmux/pull/148))

## [0.0.206] - 2026-11-16

### Added

- **Popups.** `btmux popup` runs a program in a terminal over its pane and
  closes it when the program exits; `[[popup]]` binds one to a key.
  ([#131](https://github.com/btmux-dev/btmux/pull/131))
- **Agent state from OSC 7501.** Programs can report their own state with
  program status records. ([#132](https://github.com/btmux-dev/btmux/pull/132))
- A menu to shut down the editors, and drag and drop in the desktop app.
  ([#129](https://github.com/btmux-dev/btmux/pull/129))

### Changed

- The desktop picker's color scheme matches its window or the system.
  ([#128](https://github.com/btmux-dev/btmux/pull/128))

### Fixed

- VS Code's lifecycle, settings sharing, and the window files open in.
  ([#130](https://github.com/btmux-dev/btmux/pull/130))
- Unrendered tiles no longer break the agent view's layout.
  ([#127](https://github.com/btmux-dev/btmux/pull/127))
- Windows builds no longer include Unix-only upgrade and VS Code items.
  ([#133](https://github.com/btmux-dev/btmux/pull/133))

## [0.0.205] - 2026-11-15

### Added

- Sign up for btmux connect with an invite code.
  ([#124](https://github.com/btmux-dev/btmux/pull/124))

### Changed

- The sign-in page points to enrolling when no passkey exists.
  ([#125](https://github.com/btmux-dev/btmux/pull/125))

## [0.0.204] - 2026-11-15

### Added

- **btmux connect**, in progress: a tunnel to a relay, passkey sign-in, machine
  certificates, and its status and passkeys in Settings.
  ([#115](https://github.com/btmux-dev/btmux/pull/115),
  [#117](https://github.com/btmux-dev/btmux/pull/117),
  [#119](https://github.com/btmux-dev/btmux/pull/119),
  [#122](https://github.com/btmux-dev/btmux/pull/122))
- Starfish, jellyfish, and seahorses in the aquarium wallpaper.
  ([#113](https://github.com/btmux-dev/btmux/pull/113))

### Changed

- Splitting a zoomed pane unzooms it.
  ([#121](https://github.com/btmux-dev/btmux/pull/121))
- Embedded assets are cached and compressed.
  ([#112](https://github.com/btmux-dev/btmux/pull/112))
- The access token is never accepted over a relay.
  ([#116](https://github.com/btmux-dev/btmux/pull/116))

## [0.0.203] - 2026-11-14

### Added

- Color pickers on Settings' color fields.
  ([#107](https://github.com/btmux-dev/btmux/pull/107))
- A pixel-art aquarium wallpaper shader.
  ([#108](https://github.com/btmux-dev/btmux/pull/108))

## [0.0.202] - 2026-11-13

### Added

- Several desktop app windows.
  ([#105](https://github.com/btmux-dev/btmux/pull/105))

## [0.0.201] - 2026-11-13

### Added

- Settings' Save writes to `config.toml`.
  ([#104](https://github.com/btmux-dev/btmux/pull/104))

### Changed

- Toasts with an action stay until dismissed.
  ([#103](https://github.com/btmux-dev/btmux/pull/103))

## [0.0.200] - 2026-11-12

### Changed

- The GitHub badge shows for every branch on GitHub, with or without a pull
  request. ([#102](https://github.com/btmux-dev/btmux/pull/102))

## [0.0.199] - 2026-11-12

### Added

- Pull request and CI status in the pane corner.
  ([#100](https://github.com/btmux-dev/btmux/pull/100))
- Local and remote branches side by side in the branch popup.
  ([#101](https://github.com/btmux-dev/btmux/pull/101))

## [0.0.198] - 2026-11-10

### Added

- **Sandboxed sessions** in a shared Multipass instance.
  ([#98](https://github.com/btmux-dev/btmux/pull/98))
- A kill button and tooltips in the agent view.
  ([#99](https://github.com/btmux-dev/btmux/pull/99))

## [0.0.197] - 2026-11-09

### Added

- Claude Code's background shells and monitors in agent status.
  ([#97](https://github.com/btmux-dev/btmux/pull/97))

# Changelog

Notable changes in each btmux release. Releases before 0.0.197 are listed on
[GitHub](https://github.com/btmux-dev/btmux/releases).

There are no compatibility guarantees before 0.1.0: config fields, key
actions, saved state, and API types can change in any release.

## [Unreleased]

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

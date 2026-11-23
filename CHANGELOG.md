# Changelog

Notable changes in each btmux release. Releases before 0.0.197 are listed on
[GitHub](https://github.com/btmux-dev/btmux/releases).

There are no compatibility guarantees before 0.1.0: config fields, key
actions, saved state, and API types can change in any release.

## [Unreleased]

## [0.0.219] - 2026-11-23

### Added

- Keys for the native editor's next and previous file: `Alt-PageDown` and
  `Alt-PageUp`, VS Code's in the desktop app, and `gt`/`gT`, `]b`/`[b`,
  `:bn`, and `:bp` in Vim mode.
  ([#197](https://github.com/btmux-dev/btmux/pull/197))

### Fixed

- Reopening Git mode no longer briefly shows the last status, or drops popup
  keys like `%` typed before the new status loads.
  ([#196](https://github.com/btmux-dev/btmux/pull/196))

## [0.0.218] - 2026-11-22

### Changed

- The native editor's find and replace (`Mod-f`) uses btmux's own controls,
  with toggles for case, whole words, and regular expressions; `Alt-Enter`
  selects every match and `Mod-Alt-Enter` replaces them all.
  ([#193](https://github.com/btmux-dev/btmux/pull/193))

### Fixed

- Holding Cmd (Ctrl outside macOS) over a link a program prints, as Claude
  Code does, shows the pointer and underline.
  ([#192](https://github.com/btmux-dev/btmux/pull/192))
- Language servers whose prompts need their answer's details, like nil's
  offer to fetch missing flake inputs, ask in the native editor instead of
  warning that the client can't confirm.
  ([#195](https://github.com/btmux-dev/btmux/pull/195))

## [0.0.217] - 2026-11-22

### Added

- The status bar shows the free space on the server's home volume beside
  memory. ([#191](https://github.com/btmux-dev/btmux/pull/191))

### Changed

- The key help (`<prefix> + ?`) has tabs: the prefix's key bindings, the
  VS Code-style shortcuts, and the native editor's keys.
  ([#189](https://github.com/btmux-dev/btmux/pull/189))

### Fixed

- Window grid panes have rounded corners and a gap between them, as in the
  session switcher, and the status bar's critical CPU and memory levels use
  the color scheme's red.
  ([#190](https://github.com/btmux-dev/btmux/pull/190))
- Color schemes may use three-digit colors (`#a24`); high CPU load no longer
  stays yellow with them.
  ([#190](https://github.com/btmux-dev/btmux/pull/190))

## [0.0.216] - 2026-11-21

### Added

- Antigravity CLI (`agy`) gets what Claude Code and Codex have: `btmux skill
setup` installs its status hooks, the btmux skill, and the hook that tells
  it its pane, and agent view's Usage tab shows its plan limits. The About
  dialog's Health tab lists it.
  ([#188](https://github.com/btmux-dev/btmux/pull/188))
- The About dialog's Health tab shows each check as it finishes, instead of
  waiting for the slowest.
  ([#188](https://github.com/btmux-dev/btmux/pull/188))
- `Cmd+Shift+P` (`Ctrl+Shift+P` on Windows and Linux) opens the command
  palette, as in VS Code.
  ([#182](https://github.com/btmux-dev/btmux/pull/182))
- More VS Code keys work without the prefix: `Cmd+P` quick open,
  `Cmd+Shift+E` the file browser, `Ctrl+Shift+G` Git mode, ``Ctrl+Shift+` ``
  a new window, and `Cmd+,` settings; in a terminal, `Cmd+F` searches and
  `Cmd+\` splits. On Windows and Linux, the `Ctrl+Shift` keys and `Ctrl+,`
  work, and `Ctrl+Shift+5` splits. `vscode-keys = false` turns them off.
  ([#187](https://github.com/btmux-dev/btmux/pull/187))
- Panes grow and shrink into place when they split, close, or change layout.
  The programs in them resize once, at the start, so prompts don't flicker.
  ([#182](https://github.com/btmux-dev/btmux/pull/182))
- `btmux events` prints server events as they happen: sessions, windows, and
  panes created, renamed, selected, or closed, agent state changes,
  notifications, and widgets and popups, one line each or as JSON with
  `--json`. `GET /api/events` streams the same.
  ([#185](https://github.com/btmux-dev/btmux/pull/185))
- `btmux skill setup` sets up Claude Code, Codex, and Gemini CLI: the hooks
  that report their state, and for Claude Code and Codex a skill that teaches
  them the btmux CLI and a hook that tells them which pane they run in. The
  skill and that hook take their text from the installed btmux, so upgrading
  it updates them. `btmux skill show` prints the guide.
  ([#185](https://github.com/btmux-dev/btmux/pull/185))
- `new-window` and `split-window` run a command in the new pane, and with
  `--wait` exit with its status; `--keep-open` keeps the pane and its output
  after the command exits. `btmux wait-exit` waits for any pane's program to
  exit and exits with its status. REST and MCP get the same, through
  `command` and `keep_open` and `GET /api/panes/{id}/exit` or
  `wait_for_exit`. ([#183](https://github.com/btmux-dev/btmux/pull/183))
- Settings' **Show TOML** panel can show the settings as Home Manager's
  `programs.btmux.settings` in Nix, ready to paste.
  ([#180](https://github.com/btmux-dev/btmux/pull/180))
- `[native-editor] virtual-text`, also in Settings as **Inline diagnostics**,
  shows each line's diagnostic at the end of the line.
  ([#181](https://github.com/btmux-dev/btmux/pull/181))
- `btmux forward` forwards a TCP port that a pane's program listens on, such
  as a dev server, to a local port on your computer.
  ([#184](https://github.com/btmux-dev/btmux/pull/184))

### Changed

- Panes, and the editors and browser shown over them, have rounded corners
  and a small gap between them and around the pane area.
  ([#182](https://github.com/btmux-dev/btmux/pull/182))
- Agent view matches the rounded panes: its live panes have rounded corners,
  and the agent list has no divider and lines up with the title and tabs.
  ([#186](https://github.com/btmux-dev/btmux/pull/186))
- `btmux install-claude-code-hooks`, `install-codex-hooks`, and
  `install-gemini-cli-hooks` are replaced by `btmux skill setup`, which
  installs the same hooks and can also show and remove them.
  ([#185](https://github.com/btmux-dev/btmux/pull/185))
- Python files in the native editor get ty and ruff instead of pyright, so
  formatting works: ruff formats and lints, ty checks types.
  ([#181](https://github.com/btmux-dev/btmux/pull/181))
- A native editor file with several language servers gets features from all
  of them: diagnostics, code actions, hovers, inlay hints, code lenses, and
  completions from each, and formatting and the rest from the first that can.
  Before, only the first server was used.
  ([#181](https://github.com/btmux-dev/btmux/pull/181))
- The native editor's close button is on each tab, so inactive tabs close
  too; a middle click or `Delete` on a focused tab closes a tab as well.
  ([#181](https://github.com/btmux-dev/btmux/pull/181))
- **Show TOML** shows the whole file Save would write, keeping settings the
  dialog doesn't have. It no longer adds `allow-transparency = false` when you
  add a wallpaper without touching the setting.
  ([#180](https://github.com/btmux-dev/btmux/pull/180))

### Fixed

- `btmux skill setup` no longer sets up Gemini CLI when only Antigravity CLI,
  which shares its `~/.gemini` directory, is installed.
  ([#188](https://github.com/btmux-dev/btmux/pull/188))

## [0.0.215] - 2026-11-20

### Added

- `popup-opacity` and `popup-blur` set the native editor's popups (completions,
  hovers, pickers); `popup-blur` also blurs behind Neovim floats. Popups are
  now 85% opaque, up from 50%.
  ([#178](https://github.com/btmux-dev/btmux/pull/178))
- The native editor's status line marks each language server with a dot:
  green when ready, pulsing while starting or working, yellow when not
  responding, red when it failed. A server silent for its `timeout`
  (`[language-server.NAME]`, default 20 s) while starting now fails instead
  of starting forever.
  ([#178](https://github.com/btmux-dev/btmux/pull/178))
- Settings has a search field that finds settings across all tabs. Typing
  anywhere in the dialog starts a search.
  ([#179](https://github.com/btmux-dev/btmux/pull/179))
- `toolbar-opacity`, `toolbar-blur`, and `toolbar-saturate` set the side bar
  and status bar's background, blur, and saturation, with sliders in
  Settings → Effects.
  ([#179](https://github.com/btmux-dev/btmux/pull/179))

### Changed

- Settings no longer blurs or dims the app behind it, so changes preview
  unobscured; the "Clear background" switch is gone.
  ([#179](https://github.com/btmux-dev/btmux/pull/179))

### Fixed

- Agent view shows live panes of mixed heights as large as they fit, and when
  they can't fit, scrolls back to the first row instead of cutting off its top.
  ([#186](https://github.com/btmux-dev/btmux/pull/186))
- A new pane no longer flashes an opaque background and "Connecting…" while
  it connects. The label shows only when connecting takes over a second.
  ([#182](https://github.com/btmux-dev/btmux/pull/182))
- Dragging a pane divider no longer makes the shell prompt flicker: terminals
  resize once, when the divider is released, and the layout no longer jumps
  back while the server confirms it.
  ([#182](https://github.com/btmux-dev/btmux/pull/182))
- The process viewer's port list shows a port's addresses one per line and
  never breaks one mid-address. Short columns fit their values; long
  addresses and process names end in an ellipsis, with the full text on hover.
  A list wider than its panel scrolls sideways with the arrow keys; `h` and
  `l` still fold and unfold.
  ([#179](https://github.com/btmux-dev/btmux/pull/179))
- The TOML Settings shows no longer writes `editor = "native"`, the
  default, or leaves out `editor = "neovim"`.
  ([#179](https://github.com/btmux-dev/btmux/pull/179))
- A native editor tab whose language server crashed while starting gets its
  features once the server restarts, without a reload.
  ([#178](https://github.com/btmux-dev/btmux/pull/178))
- The native editor highlights Python stubs (`.pyi`), JSONC with comments,
  zsh files, shell startup files (`.bashrc`, `.zshrc`, …), `.gitconfig`,
  `Cargo.lock`, and `flake.lock`; file previews highlight these too, as well
  as `.mts`, `.cts`, `.htm`, LESS, and more C++ extensions.
  ([#177](https://github.com/btmux-dev/btmux/pull/177))

## [0.0.214] - 2026-11-19

### Added

- A **waiting** agent state: an agent at its prompt with background shells or
  monitors, which resumes by itself once they end, shows `waiting on 1 shell`
  instead of `working · 1 shell` or `idle · 1 shell`. `btmux wait` and
  `wait_for_agent` keep waiting through it, and its "Done" notification is
  held until the turn it resumes notifies instead.
  ([#176](https://github.com/btmux-dev/btmux/pull/176))

## [0.0.213] - 2026-11-19

### Added

- `Ctrl-n` and `Ctrl-p` (as in Vim) and `Ctrl-j` and `Ctrl-k` (as in fzf,
  for browsers that keep `Ctrl-n`) move through the native editor's
  completions, quick open, and other pickers.
  ([#174](https://github.com/btmux-dev/btmux/pull/174))

### Changed

- Searches and filters use smart case, as pane search already did: a query
  with an uppercase letter matches case, any other ignores it. This covers the
  file browser's search and filters, Git mode, quick open and the native
  editor's pickers, the session switcher, the command palette, the process
  viewer, and `btmux pick` menus. Content search used to always match case.
  ([#175](https://github.com/btmux-dev/btmux/pull/175))

- The native editor's completions use the editor's font and show a colored
  icon for each kind (field, struct, module, …), the ones the Neovim surface
  shows, instead of CodeMirror's symbols, which gave fields and properties an
  empty-looking square.
  ([#174](https://github.com/btmux-dev/btmux/pull/174))

## [0.0.212] - 2026-11-19

### Added

- An [API reference](https://btmux-dev.github.io/btmux/automation/api/) for the
  REST API, generated from its OpenAPI document, which servers serve at
  `/api/openapi.json`.
  ([#171](https://github.com/btmux-dev/btmux/pull/171))
- The key help (`<prefix> + ?`) lists the native editor's keys, and the
  command palette shows them beside each editor command, for Vim mode and
  your own mappings too.
  ([#173](https://github.com/btmux-dev/btmux/pull/173))

### Changed

- The native editor's popups (quick open, pickers, completions, hovers, the
  which-key popup, and Vim's `:` and `/` lines) are translucent and blur the
  text behind them, which they now float over.
  ([#172](https://github.com/btmux-dev/btmux/pull/172))
- Completion signatures are highlighted in the file's language.
  ([#172](https://github.com/btmux-dev/btmux/pull/172))

### Fixed

- Vim's cursor in the native editor keeps its full height while a `:` or `/`
  line is open. ([#172](https://github.com/btmux-dev/btmux/pull/172))

## [0.0.211] - 2026-11-18

### Added

- Vim mode in the native editor has Neovim's language server mappings (`grn`,
  `gra`, `grr`, `gri`, `grt`, `grx`, `gO`, `K`, `]d`),
  `gd`, `gD`, `gW`, `gq` formatting through the server, and `<leader>f`.
  ([#169](https://github.com/btmux-dev/btmux/pull/169))
- A `<leader>` (Space by default) and your own Vim mappings to any action or
  keys, in `[native-editor.vim-keys]`.
  ([#169](https://github.com/btmux-dev/btmux/pull/169))
- A which-key popup lists the keys that can follow a pending prefix, in Vim
  mode and after VS Code's `Mod+K` chords. The status line shows pending keys.
  ([#169](https://github.com/btmux-dev/btmux/pull/169))
- Go back and forward across files: `Ctrl+O` / `Ctrl+I` in Vim mode, VS Code's
  keys otherwise.
  ([#169](https://github.com/btmux-dev/btmux/pull/169))
- More of VS Code's keys in the native editor: `Mod+K` chords for hover,
  formatting, and folding, `Shift+F8`, expand and shrink selection,
  `Ctrl+G`, `Mod+Shift+Enter`, `Mod+F2`, and, in the desktop app, `Mod+T` and
  `Mod+W`.
  ([#169](https://github.com/btmux-dev/btmux/pull/169))
- `editor-*` actions for definitions, references, rename, hover, formatting,
  code lenses, diagnostics, folding, and going back and forward.
  ([#169](https://github.com/btmux-dev/btmux/pull/169))

### Changed

- The native editor's Vim hunk mappings start with the leader, Space, instead
  of `\`.
  ([#169](https://github.com/btmux-dev/btmux/pull/169))
- In the macOS desktop app, Close Window is `Cmd+Shift+W`: `Cmd+W` closes the
  native editor's file, and the window elsewhere.
  ([#169](https://github.com/btmux-dev/btmux/pull/169))
- Language servers' output, starts, and exits go to `lsp.log.YYYY-MM-DD`
  beside btmux's own log (the last 14 are kept), rather than to `btmux.log`
  at the `debug` level only.
  ([#168](https://github.com/btmux-dev/btmux/pull/168))

### Fixed

- An agent notification's **View pane** button closes its toast.
  ([#170](https://github.com/btmux-dev/btmux/pull/170))
- Option chords in the native editor on macOS, such as `Shift+Option+F` to
  format.
  ([#169](https://github.com/btmux-dev/btmux/pull/169))
- Vim's `gg` and `G` after switching files in the native editor.
  ([#169](https://github.com/btmux-dev/btmux/pull/169))

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

# Editors

`<prefix> + e` opens the configured `editor` over the active pane, and the file
browser and Git mode open files there. Set `editor` in
[`config.toml`](configuration.md):

- `"native"` (the default): btmux's own editor. Any number of panes can show
  one, each with its open files as tabs. Every browser tab shows the same files
  and edits the same text, live; unsaved changes live on the server, so a
  reload keeps them. `Mod-s` saves, a quick double Esc hides the editor, and
  files changed on disk reload unless you have unsaved changes, which get a
  warning instead.
- `"neovim"`: a Neovim GUI drawn by btmux, with your config and with its command
  line, completion menu, messages, LSP signature help, and floating windows
  rendered as native UI. Needs Neovim 0.12 or newer on the server's `PATH`;
  see [Neovim GUI](neovim.md).
- `"vscode"`: VS Code in the browser, using btmux's color scheme and font; see
  [VS Code](vscode.md).
- `"pane"`: the terminal: the pane's running Neovim, else `$EDITOR` in its
  shell.

Whatever `editor` says, the status bar shows a button for each editor the
server can run: the native editor, Neovim 0.12+, and, outside Windows, the `[vscode] server`
program, found on the server's `PATH`, else your shell's (a service's `PATH` may
be shorter), or, for VS Code on macOS, in `/Applications`. The configured editor comes first. The `editor-native`, `editor-neovim`, and
`editor-vscode` actions, unbound by default, toggle one of them from the
command palette or a `[keys]` binding, and `btmux open-editor --kind vscode`
opens one from a pane.

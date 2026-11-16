# Editors

`<prefix> + e` opens the configured `editor` over the active pane, and the file
browser and Git mode open files there. Set `editor` in
[`config.toml`](configuration.md):

- `"native"` (the default): btmux's own editor. Any number of panes can show
  one, each with its open files as tabs. Every browser tab shows the same files
  and edits the same text, live; unsaved changes live on the server, so a
  reload keeps them. `Mod-s` saves, a quick double Esc hides the editor, and
  files changed on disk reload unless you have unsaved changes, which get a
  warning instead. Language servers give it diagnostics, completion, hover,
  signature help, go to definition (F12 or Cmd/Ctrl-click), references
  (Shift-F12), rename across files (F2), and formatting (Shift-Alt-F, or on
  save with `[native-editor] format-on-save = true`); see below.
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

## Language servers

The native editor starts a language server for each file it opens, one per
server and project root (the nearest directory with one of the language's root
markers, such as `Cargo.toml`, within the Git worktree). Servers are found on
the server's `PATH`, else your shell's. Built in: rust-analyzer,
typescript-language-server (TypeScript and JavaScript), pyright, gopls, clangd,
lua-language-server, nil (Nix), bash-language-server, taplo (TOML),
yaml-language-server, and vscode-langservers-extracted (JSON, CSS, HTML).
Every browser tab shares the servers. The status line shows each server's state
and progress. Add or replace languages and servers in `config.toml`:

```toml
[[language]]
name = "python"
file-types = ["py", "pyi"]
roots = ["pyproject.toml", "setup.py"]
language-servers = ["basedpyright"]

[language-server.basedpyright]
command = "basedpyright-langserver"
args = ["--stdio"]
config = { basedpyright = { analysis = { typeCheckingMode = "strict" } } }
```

`config` answers the server's `workspace/configuration` requests; changes
apply to servers started after them.

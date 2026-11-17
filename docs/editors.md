# Editors

`<prefix> + e` opens the configured `editor` over the active pane, and the file
browser and Git mode open files there. Set `editor` in
[`config.toml`](configuration.md):

- `"native"` (the default): btmux's own editor. Any number of panes can show
  one, each with its open files as tabs. Every browser tab shows the same files
  and edits the same text, live; unsaved changes live on the server, so a
  reload keeps them. `Mod-s` saves, a quick double Esc hides the editor, and
  files changed on disk reload unless you have unsaved changes, which get a
  warning instead. Language servers give it its IDE features; see below.
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
apply to servers started after them. The Health tab of About lists the
configured servers and whether btmux finds them. Files in a sandboxed session's
mounts get servers inside its Multipass instance, found by a login shell there.

What servers provide, where they support it:

| Feature | Keys |
| --- | --- |
| Diagnostics (pushed or pulled), semantic highlighting, inlay hints, highlights of the symbol under the cursor, code lenses, folding | |
| Completion and signature help | as you type |
| Hover | the mouse |
| Go to definition | F12, or Cmd-click (Ctrl-click elsewhere) |
| References | Shift-F12 |
| Rename, across files | F2 |
| Code actions (quick fixes, refactorings) | Mod-. |
| Symbols in the file | Mod-Shift-O |
| Symbols in the workspace | Mod-Alt-O |
| Calls to the symbol | Shift-Alt-H |
| Grow the selection to the enclosing syntax | Mod-I (Mod-U shrinks it back) |
| Format the document, or the selection | Shift-Alt-F; on save with `[native-editor] format-on-save = true` |

Mod is Cmd on macOS, Ctrl elsewhere. The command palette has these as `editor-*`
actions (unbound by default; bind them in `[keys]`), plus calls from the
symbol, supertypes, and subtypes. Renames and code actions change files that
aren't open too: they wait in the editor, unsaved, under "Other unsaved files".
A server's questions come up in the browser tab you're using.

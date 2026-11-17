# Editors

`<prefix> + e` opens the configured `editor` over the active pane, and the file
browser and Git mode open files there. Set `editor` in
[`config.toml`](configuration.md):

- `"native"` (the default): btmux's own editor. Any number of panes can show
  one, each with its open files as tabs. Every browser tab shows the same files
  and edits the same text, live; unsaved changes live on the server, so a
  reload keeps them. `Mod-s` saves, `Alt-w` closes the file (asking to save
  unsaved changes), a quick double Esc hides the editor, and
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

## Vim mode

`[native-editor] vim = true` (also in Settings) gives the native editor Vim
keybindings ([codemirror-vim](https://github.com/replit/codemirror-vim)).
Turning it on or off applies to open editors at once.

```toml
[native-editor]
vim = true
```

- The status line shows the mode (NORMAL, INSERT, VISUAL, …), and normal mode
  has a block cursor in the theme's cursor color.
- `:w` saves (`:w!` overwrites changes made on disk), `:q` hides the editor,
  keeping unsaved changes, `:wq` or `:x` save and hide it, and `:bd` closes the
  file (asking to save unsaved changes; `:bd!` discards them).
- Esc leaves insert and visual mode. A quick double Esc hides the editor only
  when it starts in normal mode with no command pending, so Escs that leave
  insert mode never hide it.
- With a language server: `gd` goes to the definition, `K` shows the hover,
  and `]d`/`[d` move to the next/previous diagnostic.
- Ctrl chords (`Ctrl-d`, `Ctrl-u`, `Ctrl-o`, `Ctrl-v`, `Ctrl-r`, …) go to Vim
  rather than the editor's own keymap or the browser, except the btmux prefix
  and shortcuts browsers keep (such as Ctrl-W and Ctrl-T outside macOS).
- Mode and registers are per browser tab; edits are shared as always.

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
apply to servers started after them. nil releases since 2025-06-13 format with
nixfmt by default (Nixpkgs builds in its own); for older ones, or another
formatter, set `config = { nil = { formatting = { command = ["alejandra"] } } }`
under `[language-server.nil]`, along with `command = "nil"`. The Health tab of
About lists the configured servers and whether btmux finds them. Files in a
sandboxed session's mounts get servers inside its Multipass instance, found by
a login shell there.

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

## Git signs

In a Git worktree, the native editor marks lines that differ from the Git
index, as [gitsigns.nvim](https://github.com/lewis6991/gitsigns.nvim) does: a
green bar for added lines, blue for changed ones, and a red marker where lines
were removed. Untracked files show every line as added; ignored files show
nothing. The signs follow your edits as you type and the index as it changes,
whether btmux or `git` changes it. The status line counts added, changed, and
removed lines.

Click a sign to preview its hunk inline: the index's lines above the buffer's,
with the changed words marked, and buttons to stage or reset the hunk.

| Action | Keys | Vim |
| --- | --- | --- |
| Next / previous hunk | Alt-F5 / Shift-Alt-F5 | `]c` / `[c` |
| Preview the hunk | click its sign | `\hp` |
| Stage the hunk (or the selection's hunks) | | `\hs` |
| Reset the hunk to the index | | `\hr` |
| Stage the whole buffer | | `\hS` |
| Reset the whole buffer | | `\hR` |
| Blame the line, with the full commit message | | `\hb` |
| Toggle blame at the end of the cursor line | | `\tb` |
| The hunk's lines as a text object (`dih`, `vih`) | | `ih` |

Staging writes the buffer's text to the index, saved or not, as gitsigns does;
resetting edits the buffer, so undo brings the change back. The command palette
has each as an `editor-*` action (`editor-next-hunk`, `editor-stage-hunk`,
`editor-toggle-line-blame`, …), unbound by default; bind them in `[keys]`.
Line blame stays on or off per browser.

// The native editor's own Vim ex commands: `vim.ts` defines one handler per
// entry, and the key help lists those with a label. Kept apart from `vim.ts`
// so the key help doesn't load codemirror-vim.

export interface VimCommand {
  /** The shortest abbreviation. */
  prefix: string;
  /** The key help's row; commands sharing one share the row. */
  label?: string;
  /** How the key help writes it, if not `:prefix`. */
  shown?: string;
}

export const VIM_COMMANDS = {
  write: { prefix: 'w', label: 'Save' },
  quit: { prefix: 'q', label: 'Hide the editor' },
  qall: { prefix: 'qa', label: 'Hide the editor' },
  wq: { prefix: 'wq', label: 'Save and hide the editor' },
  xit: { prefix: 'x', label: 'Save and hide the editor' },
  bdelete: { prefix: 'bd', label: 'Close the file' },
  // As vim-bufonly's.
  BufOnly: { prefix: 'BufOnly', label: 'Close the other files' },
  bnext: { prefix: 'bn', label: 'Next file' },
  bprevious: { prefix: 'bp', label: 'Previous file' },
  // The problems list is the quickfix list.
  copen: { prefix: 'cope', label: 'Show the problems list', shown: ':copen' },
  cclose: { prefix: 'ccl', label: 'Hide the problems list', shown: ':cclose' },
  cwindow: { prefix: 'cw', label: 'Show the problems list if there are any' },
  // The location list, as `vim.diagnostic.setloclist()` fills it.
  lopen: { prefix: 'lop', label: "Show the file's problems", shown: ':lopen' },
  lclose: { prefix: 'lcl', label: "Hide the file's problems", shown: ':lclose' },
  lwindow: { prefix: 'lw', label: "Show the file's problems if there are any" },
} satisfies Record<string, VimCommand>;

export type VimCommandName = keyof typeof VIM_COMMANDS;

/** The key help's rows, in table order. */
export function vimCommandRows(): { label: string; keys: string[] }[] {
  const rows = new Map<string, string[]>();
  for (const command of Object.values(VIM_COMMANDS) as VimCommand[]) {
    if (!command.label) continue;
    const keys = rows.get(command.label) ?? rows.set(command.label, []).get(command.label)!;
    keys.push(command.shown ?? `:${command.prefix}`);
  }
  return [...rows].map(([label, keys]) => ({ label, keys }));
}

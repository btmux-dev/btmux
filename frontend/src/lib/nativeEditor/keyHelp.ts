// The native editor's keys by action, for the command palette and key help:
// VS Code's without vim, the config's vim mappings with it. The editor's
// keymaps bind the same keys; a browser test checks they agree.
import type { PopupBinding, VimBind, VimMode } from '../../generated/protocol';
import { actionLabel } from '../actionLabel';
import { isDesktopApp } from '../desktopApp';
import { IS_MAC } from '../nvimKeys';
import { keyCapKeys, showKeys } from './keyLayer';
import { EDITOR_COMMANDS } from './registry';

const IS_WINDOWS = typeof navigator !== 'undefined' && /^Win/.test(navigator.platform);

/** VS Code's Go Back and Go Forward. */
export const NAVIGATION_KEYS = IS_MAC
  ? { back: 'Ctrl--', forward: 'Ctrl-Shift--' }
  : IS_WINDOWS
    ? { back: 'Alt-ArrowLeft', forward: 'Alt-ArrowRight' }
    : { back: 'Ctrl-Alt--', forward: 'Ctrl-Shift--' };

/**
 * Next and previous file: VS Code's in the desktop app, as browsers keep
 * them for their own tabs; `Alt-PageDown`/`Alt-PageUp` everywhere.
 */
export const fileKeys = () => ({
  next: ['Alt-PageDown', ...(isDesktopApp() ? ['Ctrl-Tab', IS_MAC ? 'Cmd-Alt-ArrowRight' : 'Ctrl-PageDown'] : [])],
  prev: ['Alt-PageUp', ...(isDesktopApp() ? ['Ctrl-Shift-Tab', IS_MAC ? 'Cmd-Alt-ArrowLeft' : 'Ctrl-PageUp'] : [])],
});

/** VS Code's `Mod-k` chords, as codemirror-vim key names. */
export const PLAIN_CHORDS: [string[], string][] = [
  [['k', 'i'], 'editor-hover'],
  [['k', 'f'], 'editor-format'],
  [['k', '0'], 'editor-fold-all'],
  [['k', 'j'], 'editor-unfold-all'],
].map(([keys, action]) => [(keys as string[]).map((k) => `<${IS_MAC ? 'M' : 'C'}-${k}>`), action as string]);

/**
 * Single keys of actions without vim, in CodeMirror's notation, as the
 * keymaps bind them on this platform.
 */
export function plainKeys(): Record<string, string[]> {
  return {
    'editor-quick-open': ['Mod-p'],
    'editor-go-to-definition': ['F12'],
    'editor-find-references': ['Shift-F12'],
    'editor-go-to-implementation': ['Mod-F12'],
    'editor-rename': ['F2'],
    'editor-signature-help': ['Mod-Shift-Space'],
    'editor-code-actions': ['Mod-.'],
    'editor-format': ['Shift-Alt-f'],
    'editor-document-symbols': ['Mod-Shift-o'],
    'editor-workspace-symbols': ['Mod-Alt-o', ...(isDesktopApp() ? ['Mod-t'] : [])],
    'editor-incoming-calls': ['Shift-Alt-h'],
    'editor-expand-selection': [IS_MAC ? 'Ctrl-Shift-Cmd-ArrowRight' : 'Shift-Alt-ArrowRight', 'Mod-i'],
    'editor-shrink-selection': [IS_MAC ? 'Ctrl-Shift-Cmd-ArrowLeft' : 'Shift-Alt-ArrowLeft'],
    'editor-next-diagnostic': ['F8'],
    'editor-prev-diagnostic': ['Shift-F8'],
    'editor-problems': ['Mod-Shift-m'],
    'editor-go-back': [NAVIGATION_KEYS.back],
    'editor-go-forward': [NAVIGATION_KEYS.forward],
    'editor-next-file': fileKeys().next,
    'editor-prev-file': fileKeys().prev,
    'editor-next-hunk': ['Alt-F5'],
    'editor-prev-hunk': ['Shift-Alt-F5'],
  };
}

/** Keys without an action, without vim. */
export function plainBasics(): { label: string; keys: string[] }[] {
  return [
    { label: 'Save', keys: ['Mod-s'] },
    { label: 'Close the file', keys: ['Alt-w', ...(isDesktopApp() ? ['Mod-w'] : [])] },
    { label: 'Go to line', keys: ['Ctrl-g'] },
    { label: 'Select the next occurrence', keys: ['Mod-d'] },
    { label: 'Select every occurrence', keys: ['Mod-F2'] },
    { label: 'Insert a line above', keys: ['Mod-Shift-Enter'] },
  ];
}

/** One way to type something: key caps in order, and the vim modes it needs if not normal mode. */
export interface EditorKey {
  caps: string[];
  modes?: VimMode[];
}

const CM_MODIFIERS: Record<string, string> = {
  Mod: 'mod',
  Cmd: 'mod',
  Meta: 'mod',
  Ctrl: 'ctrl',
  Alt: 'alt',
  Shift: 'shift',
};
const CM_NAMES: Record<string, string> = {
  ArrowLeft: 'left',
  ArrowRight: 'right',
  ArrowUp: 'up',
  ArrowDown: 'down',
  Space: 'space',
  Enter: 'enter',
  Escape: 'esc',
};

/** A `KeyCap` name for a CodeMirror key: `Mod-Shift-o` is `mod+shift+o`. */
export function cmKeyCap(key: string): string {
  const m = /^((?:(?:Mod|Cmd|Meta|Ctrl|Alt|Shift)-)*)(.+)$/.exec(key)!;
  const mods = m[1]
    .split('-')
    .filter(Boolean)
    .map((mod) => CM_MODIFIERS[mod]);
  return [...mods, CM_NAMES[m[2]] ?? m[2]].join('+');
}

/** Key caps for vim key names: runs of typed characters share one (`grn`, `␣ hs`). */
export function vimKeyCaps(keys: readonly string[]): string[] {
  const caps: string[] = [];
  let run = '';
  for (const key of keys) {
    if (key.length === 1) {
      run += key;
      continue;
    }
    if (run) caps.push(run);
    run = '';
    caps.push(keyCapKeys(key));
  }
  if (run) caps.push(run);
  return caps;
}

const chordKeys = (action: string): EditorKey[] =>
  PLAIN_CHORDS.filter(([, a]) => a === action).map(([keys]) => ({ caps: vimKeyCaps(keys) }));

/**
 * Vim mappings, one per key sequence and target, with every mode it's in;
 * those in normal mode first.
 */
function vimKeys(binds: readonly VimBind[]): { bind: VimBind; key: EditorKey }[] {
  const byKeys = new Map<string, { bind: VimBind; modes: VimMode[] }>();
  for (const bind of binds) {
    const id = [bind.keys, bind.action, bind.rhs].map((part) => JSON.stringify(part)).join(' ');
    const entry = byKeys.get(id) ?? byKeys.set(id, { bind, modes: [] }).get(id)!;
    entry.modes.push(bind.mode);
  }
  return [...byKeys.values()]
    .map(({ bind, modes }) => ({
      bind,
      // Normal mode is where keys are typed unless said otherwise.
      key: { caps: vimKeyCaps(bind.keys), modes: modes.includes('normal') ? undefined : modes },
    }))
    .sort((a, b) => Number(!!a.key.modes) - Number(!!b.key.modes));
}

/**
 * The keys that run `action` in the editor: with vim, its mappings, else
 * the keys without vim. Without a mapping, vim takes the keys without vim
 * except VS Code's chords, whose table is only for the editor without vim.
 */
export function editorKeys(action: string, vim: { binds: readonly VimBind[] } | null): EditorKey[] {
  const plain = (plainKeys()[action] ?? []).map((key) => ({ caps: [cmKeyCap(key)] }));
  if (!vim) return [...plain, ...chordKeys(action)];
  const mapped = vimKeys(vim.binds.filter((b) => b.action === action)).map(({ key }) => key);
  return mapped.length ? mapped : plain;
}

/** Vim's commands for the editor. */
export const VIM_BASICS: { label: string; keys: string[] }[] = [
  { label: 'Save', keys: [':w'] },
  { label: 'Hide the editor', keys: [':q'] },
  { label: 'Save and hide the editor', keys: [':wq'] },
  { label: 'Close the file', keys: [':bd'] },
];

/** A row of the key help's editor section. */
export interface EditorKeyRow {
  label: string;
  keys: EditorKey[];
}

/** An action's label under the editor's heading: `editor-format` is `Format`. */
function rowLabel(action: string, popups?: PopupBinding[]): string {
  const label = actionLabel(action, popups).replace(/^Editor /, '');
  return label[0].toUpperCase() + label.slice(1);
}

/** What the key help lists for the editor, in the mode it's in. */
export function editorKeyRows(config: {
  vim_mode: boolean;
  vim_binds: readonly VimBind[];
  popups?: PopupBinding[];
}): EditorKeyRow[] {
  const vim = config.vim_mode ? { binds: config.vim_binds } : null;
  const literal = (keys: string[]): EditorKey[] => keys.map((key) => ({ caps: [key] }));
  const basics: EditorKeyRow[] = vim
    ? VIM_BASICS.map((row) => ({ label: row.label, keys: literal(row.keys) }))
    : plainBasics().map((row) => ({ label: row.label, keys: row.keys.map((key) => ({ caps: [cmKeyCap(key)] })) }));
  basics.push({ label: 'Hide the editor', keys: [{ caps: ['esc', 'esc'] }] });
  const actions = EDITOR_COMMANDS.flatMap((action) => {
    const keys = editorKeys(action, vim);
    return keys.length ? [{ label: rowLabel(action, config.popups), keys }] : [];
  });
  // The rest of the vim mappings: other btmux actions, and keys for vim.
  const other = vimKeys((vim?.binds ?? []).filter((b) => !b.action || !EDITOR_COMMANDS.includes(b.action))).map(
    ({ bind: b, key }) => ({
      label: b.desc ?? (b.action ? rowLabel(b.action, config.popups) : `Types ${showKeys(b.rhs ?? [])}`),
      keys: [key],
    }),
  );
  return [...basics, ...actions, ...other];
}

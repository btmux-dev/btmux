// What the which-key popup lists after a pending prefix: the editor's
// mappings, and codemirror-vim's own keys after `g`, `z`, `[`, `]`, `"`, or
// an operator.
import { actionLabel } from '../actionLabel';
import { groupName, type KeyEntry, type KeyTable } from './keyLayer';

export interface WhichKeyRow {
  key: string;
  label: string;
  /** More keys follow. */
  group: boolean;
}

const G: [string, string][] = [
  ['g', 'First line'],
  ['e', 'Back to word end'],
  ['E', 'Back to WORD end'],
  ['j', 'Down a display line'],
  ['k', 'Up a display line'],
  ['u', 'Lowercase'],
  ['U', 'Uppercase'],
  ['~', 'Switch case'],
  ['c', 'Comment'],
  ['q', 'Format'],
  ['w', 'Wrap lines'],
  ['n', 'Select next match'],
  ['N', 'Select previous match'],
  ['v', 'Last selection'],
  ['i', 'Insert at last edit'],
  ['I', 'Insert at line start'],
  ['J', 'Join without spaces'],
  ['?', 'Rot13'],
];

const Z: [string, string][] = [
  ['z', 'Line to center'],
  ['t', 'Line to top'],
  ['b', 'Line to bottom'],
];

const BRACKETS: Record<string, [string, string][]> = {
  ']': [
    ['p', 'Paste below, indented'],
    ["'", 'Next mark line'],
    ['`', 'Next mark'],
  ],
  '[': [
    ['p', 'Paste above, indented'],
    ["'", 'Previous mark line'],
    ['`', 'Previous mark'],
  ],
};

const MOTIONS: [string, string][] = [
  ['w', 'Next word'],
  ['b', 'Previous word'],
  ['e', 'Word end'],
  ['W', 'Next WORD'],
  ['B', 'Previous WORD'],
  ['E', 'WORD end'],
  ['0', 'Line start'],
  ['^', 'First non-blank'],
  ['$', 'Line end'],
  ['G', 'Last line'],
  ['j', 'Line down'],
  ['k', 'Line up'],
  ['f', 'To character'],
  ['t', 'Till character'],
  ['%', 'Matching bracket'],
  ['}', 'Next paragraph'],
  ['{', 'Previous paragraph'],
];

const OBJECTS: [string, string][] = [
  ['w', 'Word'],
  ['W', 'WORD'],
  ['s', 'Sentence'],
  ['p', 'Paragraph'],
  ['(', 'Parentheses'],
  ['[', 'Brackets'],
  ['{', 'Braces'],
  ['<', 'Angle brackets'],
  ['"', 'Double quotes'],
  ["'", 'Single quotes'],
  ['`', 'Backticks'],
  ['t', 'Tag'],
];

const rows = (list: [string, string][]): WhichKeyRow[] => list.map(([key, label]) => ({ key, label, group: false }));

/** A mapping's description: its own, or its action's label without "Editor". */
export function describe(entry: KeyEntry): string {
  if (entry.desc) return entry.desc;
  if (!entry.action) return (entry.rhs ?? []).join('');
  const label = actionLabel(entry.action).replace(/^Editor /, '');
  return label.charAt(0).toUpperCase() + label.slice(1);
}

/** codemirror-vim's own keys after `keys` (a count and register dropped). */
export function vimPresets(
  keys: readonly string[],
  operator: boolean,
  registers: () => { name: string; text: string }[],
): WhichKeyRow[] {
  let rest = keys.join('').replace(/^"./, '').replace(/^\d+/, '');
  if (operator) {
    // After the operator's own keys (`d`, `gu`, `>`), a count, then `i` or `a`.
    rest = rest.replace(/^(?:g[~uUcqw?]|[dcy<>=!])/, '').replace(/^\d+/, '');
    if (rest === 'i')
      return [...rows(OBJECTS), { key: 'h', label: 'Hunk', group: false }].map((r) => ({
        ...r,
        label: `Inner ${r.label.toLowerCase()}`,
      }));
    if (rest === 'a') return rows(OBJECTS).map((r) => ({ ...r, label: `Around ${r.label.toLowerCase()}` }));
    if (rest) return [];
    return [...rows(MOTIONS), { key: 'i', label: 'Inside', group: true }, { key: 'a', label: 'Around', group: true }];
  }
  if (rest === 'g') return rows(G);
  if (rest === 'z') return rows(Z);
  if (rest === ']' || rest === '[') return rows(BRACKETS[rest]);
  if (keys.length === 1 && keys[0] === '"')
    return registers().map((r) => ({
      key: r.name,
      label: r.text.replace(/\s+/g, ' ').trim().slice(0, 60),
      group: false,
    }));
  return [];
}

/** The editor's mappings after `keys`, over `presets` for the same keys. */
export function whichKeyRows(
  table: KeyTable | null,
  keys: readonly string[],
  groups: Map<string, string>,
  presets: WhichKeyRow[] = [],
): WhichKeyRow[] {
  const byKey = new Map(presets.map((r) => [r.key, r]));
  for (const { key, entry, more } of table?.next(keys) ?? []) {
    const name = more ? groupName(groups, [...keys, key]) : null;
    if (entry)
      byKey.set(key, { key, label: more ? `${describe(entry)} (+${name ?? 'more'})` : describe(entry), group: false });
    else byKey.set(key, { key, label: name ?? `${more} more`, group: true });
  }
  return [...byKey.values()].sort((a, b) => sortKey(a.key).localeCompare(sortKey(b.key)) || a.key.localeCompare(b.key));
}

// Letters by letter, lowercase first; other keys after them.
const sortKey = (key: string) =>
  /^[a-z]$/i.test(key) ? `0${key.toLowerCase()}${key === key.toLowerCase() ? 0 : 1}` : `1${key}`;

// LSP positions, ranges, and snippets, as the native editor's language
// server features share them.
import { diff } from '@codemirror/merge';
import type { ChangeSpec, Text } from '@codemirror/state';

export type Position = { line: number; character: number };
export type Range = { start: Position; end: Position };
export type Location = { uri: string; range: Range };
export type TextEdit = { range: Range; newText: string };
export type Command = { title: string; command: string; arguments?: unknown[] };

/** The offset of `pos` in `doc`, clamped to its line and the document. */
export function offset(doc: Text, pos: Position) {
  if (pos.line >= doc.lines) return doc.length;
  const line = doc.line(pos.line + 1);
  return Math.min(line.from + pos.character, line.to);
}

/** The offset of `pos` in `doc`, or null if it isn't in it. */
export function offsetChecked(doc: Text, pos: Position) {
  if (pos.line < 0 || pos.line >= doc.lines) return null;
  const line = doc.line(pos.line + 1);
  return pos.character < 0 || pos.character > line.length ? null : line.from + pos.character;
}

/** An LSP snippet in CodeMirror's syntax: `$1` as `${1}`. */
export const lspToSnippet = (text: string) =>
  text.replace(/\\([$}\\])|\$(\d+)/g, (_m, esc, field) => esc || `\${${field}}`);

/**
 * Several edits to `doc`, each as only the parts that differ
 * (`minimalChanges`). Sorted by start first (stably, keeping the order of
 * edits at one position): adjacent edits can each become an insertion at
 * their shared boundary, which apply in the order given, while LSP allows
 * edits in any order.
 */
export function minimalEdits(doc: Text, edits: { from: number; to: number; insert: string }[]): ChangeSpec[] {
  return [...edits].sort((a, b) => a.from - b.from).flatMap((e) => minimalChanges(doc, e.from, e.to, e.insert));
}

/**
 * `insert` in place of `from..to` in `doc`, as only the parts that differ:
 * formatters often replace the whole file, which would move the cursor,
 * selection, and every mark to its start.
 */
export function minimalChanges(doc: Text, from: number, to: number, insert: string): ChangeSpec[] {
  // The editor's line breaks; a lone `\r` would insert one.
  const text = insert.replace(/\r\n?/g, '\n');
  const old = doc.sliceString(from, to);
  if (old === text) return [];
  return diff(old, text, { scanLimit: 20_000, timeout: 100 }).map((c) => ({
    from: from + c.fromA,
    to: from + c.toA,
    insert: text.slice(c.fromB, c.toB),
  }));
}

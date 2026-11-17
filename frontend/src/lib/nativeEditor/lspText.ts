// LSP positions, ranges, and snippets, as the native editor's language
// server features share them.
import type { Text } from '@codemirror/state';

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

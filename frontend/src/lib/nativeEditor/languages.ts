import { LanguageDescription, LanguageSupport, StreamLanguage } from '@codemirror/language';
import { languages as dataLanguages } from '@codemirror/language-data';
import { tags } from '@lezer/highlight';

/** Extensions and whole file names language-data's languages lack. */
const EXTRA: Record<string, { extensions?: string[]; filenames?: string[] }> = {
  Python: { extensions: ['pyi'] },
  Shell: { extensions: ['zsh'], filenames: ['.bashrc', '.bash_profile', '.zshrc', '.zshenv', '.zprofile'] },
  TOML: { filenames: ['Cargo.lock'] },
  JSON: { filenames: ['flake.lock'] },
  'Properties files': { filenames: ['.gitconfig'] },
};

const escape = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

function extend(d: LanguageDescription): LanguageDescription {
  const extra = EXTRA[d.name];
  if (!extra) return d;
  const names = extra.filenames?.map(escape).join('|');
  const patterns = [d.filename?.source, names && `^(?:${names})$`].filter(Boolean);
  return LanguageDescription.of({
    name: d.name,
    alias: d.alias,
    extensions: [...d.extensions, ...(extra.extensions ?? [])],
    filename: patterns.length ? new RegExp(patterns.join('|')) : undefined,
    load: () => d.load(),
  });
}

/** language-data's languages plus ones it lacks. */
export const languages = [
  ...dataLanguages.map(extend),
  LanguageDescription.of({
    name: 'JSONC',
    extensions: ['jsonc'],
    // Lezer's JSON grammar has no comments.
    load: () =>
      import('@codemirror/legacy-modes/mode/javascript').then(
        (m) => new LanguageSupport(StreamLanguage.define({ ...m.json, tokenTable: { property: tags.propertyName } })),
      ),
  }),
  LanguageDescription.of({
    name: 'Nix',
    extensions: ['nix'],
    load: () => import('./nix').then((m) => m.nix()),
  }),
  LanguageDescription.of({
    name: 'Just',
    extensions: ['just'],
    filename: /^\.?justfile$/i,
    load: () => import('./just').then((m) => m.just()),
  }),
];

/** The language for a file name, loaded on first use. */
export async function languageFor(path: string): Promise<{ name: string; support: LanguageSupport } | null> {
  const name = path.slice(path.lastIndexOf('/') + 1);
  const description = LanguageDescription.matchFilename(languages, name);
  if (!description) return null;
  try {
    return { name: description.name, support: await description.load() };
  } catch {
    return null;
  }
}

/**
 * The indent unit `text` uses: a tab if more indented lines start with one,
 * else the most common step between space indents (2 when unsure).
 */
export function detectIndent(text: string): string {
  let tabs = 0;
  let spaces = 0;
  const steps = new Map<number, number>();
  let previous = 0;
  let lines = 0;
  for (const line of text.split('\n', 10_000)) {
    if (!line.trim()) continue;
    if (++lines > 2000) break;
    if (line[0] === '\t') {
      tabs++;
      continue;
    }
    const width = line.length - line.trimStart().length;
    if (width > 0) spaces++;
    const step = Math.abs(width - previous);
    if (step > 0 && step <= 8) steps.set(step, (steps.get(step) ?? 0) + 1);
    previous = width;
  }
  if (tabs > spaces) return '\t';
  let best = 2;
  let count = 0;
  for (const [step, n] of steps) {
    if (n > count || (n === count && step < best)) {
      best = step;
      count = n;
    }
  }
  return ' '.repeat(best);
}

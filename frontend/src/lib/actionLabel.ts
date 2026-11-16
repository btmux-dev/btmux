import type { PopupBinding } from '../generated/protocol';

const WORDS: Record<string, string> = { latex: 'LaTeX', neovim: 'Neovim', vscode: 'VS Code' };

/** The `[[popup]]` binding a `popup-<index>` action runs. */
export function popupIndex(action: string): number | null {
  const match = /^popup-(\d+)$/.exec(action);
  return match ? Number(match[1]) : null;
}

/** Sentence-case label for a kebab-case keybinding action, e.g. `split-vertical` → `Split vertical`. */
export function actionLabel(action: string, popups?: PopupBinding[]): string {
  const popup = popupIndex(action);
  if (popup !== null) {
    const binding = popups?.[popup];
    return binding ? `Popup: ${binding.title ?? binding.command}` : 'Popup';
  }
  const text = action
    .split('-')
    .map((word) => WORDS[word] ?? word)
    .join(' ');
  return text.charAt(0).toUpperCase() + text.slice(1);
}

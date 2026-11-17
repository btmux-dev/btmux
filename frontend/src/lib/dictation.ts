/**
 * iOS dictation inserts its first guess, then replaces it as it refines it, but
 * only while the text before the caret is still that guess (WebKit's
 * `replaceDictatedText`). wterm empties its textarea after each input, so
 * dictation stops after a letter or two. On iOS, inserted text stays in the
 * textarea instead, and each change reaches the PTY as backspaces and text,
 * until a key that isn't text is pressed.
 */

export const IOS =
  typeof navigator !== 'undefined' &&
  (/iPhone|iPad|iPod/.test(navigator.userAgent) ||
    (/Macintosh/.test(navigator.userAgent) && navigator.maxTouchPoints > 1));

// wterm keeps a zero-width space in a touch textarea, so Backspace on it still deletes.
const PLACEHOLDER = '​';
// wterm drops an input repeating a composition's text this soon after it.
const AFTER_COMPOSITION_MS = 250;
// Keys that leave a dictated line alone (IMEs report keys they handle as Process or Unidentified).
const KEEPS_TEXT = new Set(['Backspace', 'Shift', 'Control', 'Alt', 'Meta', 'CapsLock', 'Process', 'Unidentified']);

const segmenter = new Intl.Segmenter(undefined, { granularity: 'grapheme' });
const graphemes = (text: string) => Array.from(segmenter.segment(text), (part) => part.segment);

/**
 * Turns `before` into `after` at the end of a line: graphemes to delete, then
 * text to type. Null when that deletes a grapheme of several code points:
 * one Backspace deletes it in fish but only a code point in bash, so no count
 * is right everywhere.
 */
export function textEdit(before: string, after: string) {
  const from = graphemes(before);
  const to = graphemes(after);
  let same = 0;
  while (same < from.length && same < to.length && from[same] === to[same]) same++;
  const removed = from.slice(same);
  if (removed.some((grapheme) => [...grapheme].length > 1)) return null;
  return { deletes: removed.length, text: to.slice(same).join('') };
}

const strip = (value: string) => (value.startsWith(PLACEHOLDER) ? value.slice(PLACEHOLDER.length) : value);

export interface InsertedTextTarget {
  /** Clears the state wterm's own input handling clears, like an armed Escape → Tab exit. */
  accept(): void;
  send(text: string): void;
  paste(text: string): void;
  deleteBack(): void;
}

/** Keeps inserted text in wterm's touch textarea; returns a dispose function. */
export function keepInsertedText(element: HTMLElement, textarea: HTMLTextAreaElement, target: InsertedTextTarget) {
  let before: string | null = null;
  let composed: { text: string; at: number } | null = null;
  const setValue = (text: string) => {
    textarea.value = PLACEHOLDER + text;
    textarea.setSelectionRange(textarea.value.length, textarea.value.length);
  };
  const onBeforeInput = (event: Event) => {
    if (event.target === textarea) before = textarea.value;
  };
  const onCompositionEnd = (event: Event) => {
    const text = (event as CompositionEvent).data;
    composed = text ? { text, at: performance.now() } : null;
  };
  const onInput = (event: Event) => {
    const previous = before;
    before = null;
    if (event.target !== textarea || !(event instanceof InputEvent) || event.isComposing || previous === null) return;
    const recent = composed;
    composed = null;
    // wterm drops a repeat of the composition's text.
    if (recent && strip(textarea.value) === recent.text && performance.now() - recent.at < AFTER_COMPOSITION_MS) return;
    const type = event.inputType;
    const paste = type === 'insertFromPaste' || type === 'insertFromDrop';
    const retained = strip(previous);
    // With nothing retained, deletions and pastes stay wterm's.
    if (!retained && (paste || !type.startsWith('insert'))) return;
    event.stopImmediatePropagation();
    const edit = textEdit(retained, strip(textarea.value));
    if (!edit) {
      // Keep what the PTY has; dictation then stops, as it did before.
      setValue(retained);
      return;
    }
    target.accept();
    if (!textarea.value.startsWith(PLACEHOLDER)) {
      const caret = textarea.selectionStart + PLACEHOLDER.length;
      textarea.value = PLACEHOLDER + textarea.value;
      textarea.setSelectionRange(caret, caret);
    }
    for (let i = 0; i < edit.deletes; i++) target.deleteBack();
    if (!edit.text) return;
    if (paste) {
      // A paste empties the field, as wterm's does.
      setValue('');
      target.paste(edit.text);
    } else target.send(edit.text);
  };
  const onKeyDown = (event: KeyboardEvent) => {
    if (event.target !== textarea || event.isComposing) return;
    const modified = event.ctrlKey || event.altKey || event.metaKey;
    if (!modified && event.key === 'Backspace' && event.isTrusted && strip(textarea.value)) {
      // The textarea deletes, maybe a selection, and its input event sends that.
      event.stopImmediatePropagation();
      return;
    }
    if ([...event.key].length === 1 && !modified) return;
    if (KEEPS_TEXT.has(event.key) && !(modified && event.key === 'Backspace')) return;
    // Keys other than text end the line being dictated.
    if (textarea.value !== PLACEHOLDER) setValue('');
  };
  element.addEventListener('beforeinput', onBeforeInput, true);
  element.addEventListener('input', onInput, true);
  element.addEventListener('compositionend', onCompositionEnd, true);
  element.addEventListener('keydown', onKeyDown, true);
  return () => {
    element.removeEventListener('beforeinput', onBeforeInput, true);
    element.removeEventListener('input', onInput, true);
    element.removeEventListener('compositionend', onCompositionEnd, true);
    element.removeEventListener('keydown', onKeyDown, true);
  };
}

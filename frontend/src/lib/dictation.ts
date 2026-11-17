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
const PLACEHOLDER = '\u200b';
// wterm drops an input repeating a composition's text this soon after it.
const AFTER_COMPOSITION_MS = 250;
// Keys that leave a dictated line alone (IMEs report keys they handle as Process or Unidentified).
const KEEPS_TEXT = new Set(['Backspace', 'Shift', 'Control', 'Alt', 'Meta', 'CapsLock', 'Process', 'Unidentified']);

/** Turns `before` into `after` at the end of a line: code points to delete, then text to type. */
export function textEdit(before: string, after: string) {
  const from = [...before];
  const to = [...after];
  let same = 0;
  while (same < from.length && same < to.length && from[same] === to[same]) same++;
  return { deletes: from.length - same, text: to.slice(same).join('') };
}

const strip = (value: string) => (value.startsWith(PLACEHOLDER) ? value.slice(PLACEHOLDER.length) : value);

/** Keeps inserted text in wterm's touch textarea; returns a dispose function. */
export function keepInsertedText(
  element: HTMLElement,
  textarea: HTMLTextAreaElement,
  send: (text: string) => void,
  deleteBack: () => void,
) {
  let before: string | null = null;
  let composedAt = -Infinity;
  const clear = () => {
    if (textarea.value === PLACEHOLDER) return;
    textarea.value = PLACEHOLDER;
    textarea.setSelectionRange(1, 1);
  };
  const onBeforeInput = (event: Event) => {
    if (event.target === textarea) before = textarea.value;
  };
  const onCompositionEnd = () => {
    composedAt = performance.now();
  };
  const onInput = (event: Event) => {
    const previous = before;
    before = null;
    if (event.target !== textarea || !(event instanceof InputEvent) || event.isComposing || previous === null) return;
    const type = event.inputType;
    // Deletions and text just after a composition stay wterm's: neither sends retained text.
    if (!type.startsWith('insert') || performance.now() - composedAt < AFTER_COMPOSITION_MS) return;
    const edit = textEdit(strip(previous), strip(textarea.value));
    if (type === 'insertFromPaste' || type === 'insertFromDrop') {
      // wterm sends the textarea's text as the paste.
      textarea.value = PLACEHOLDER + edit.text;
      return;
    }
    event.stopImmediatePropagation();
    if (!textarea.value.startsWith(PLACEHOLDER)) {
      const caret = textarea.selectionStart + PLACEHOLDER.length;
      textarea.value = PLACEHOLDER + textarea.value;
      textarea.setSelectionRange(caret, caret);
    }
    for (let i = 0; i < edit.deletes; i++) deleteBack();
    if (edit.text) send(edit.text);
  };
  // Keys other than text end the line being dictated.
  const onKeyDown = (event: KeyboardEvent) => {
    if (event.target !== textarea || event.isComposing) return;
    const text = [...event.key].length === 1 && !event.ctrlKey && !event.altKey && !event.metaKey;
    if (text || KEEPS_TEXT.has(event.key)) return;
    clear();
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

import { useEffect, useState } from 'react';
import { HStack } from '@astryxdesign/core/Layout';
import { Button } from '@astryxdesign/core/Button';
import { IconButton } from '@astryxdesign/core/IconButton';
import { ToggleButton } from '@astryxdesign/core/ToggleButton';
import { ArrowDown, ArrowLeft, ArrowRight, ArrowUp, KeyboardOff, PanelBottomClose } from 'lucide-react';
import { nativeEditorInput, nativeEditorOver, useStore } from '../state/store';
import { CONFIG_DEFAULTS, getPrefix } from '../state/configDefaults';
import { parsePrefix } from '../lib/prefixKey';
import { dispatchKey, type KeyModifiers } from '../lib/paneTerminal';
import { TEXT_FIELDS } from '../hooks/usePressHint';
import type { ClientMessage } from '../protocol/messages';

const ARROWS = [
  { key: 'ArrowLeft', label: 'Left', icon: ArrowLeft },
  { key: 'ArrowDown', label: 'Down', icon: ArrowDown },
  { key: 'ArrowUp', label: 'Up', icon: ArrowUp },
  { key: 'ArrowRight', label: 'Right', icon: ArrowRight },
];

// Inputs the key bar types into: pane terminals, the built-in Neovim, the browser, and `btmux` widgets.
const takesKeys = (target: EventTarget | null) =>
  target instanceof Element &&
  target.closest(
    '[data-pane-terminal], [data-nvim-input], [data-browser-input], [data-native-editor], [data-pane-widget]',
  ) !== null;

/** iPhone keyboards have no dismiss key. */
export const hideKeyboard = () => (document.activeElement as HTMLElement | null)?.blur();

/** Keys phone keyboards lack. Ctrl and Alt apply to the next key, from here or the keyboard. */
export function MobileKeyBar({
  sessionId,
  send,
  layout = 'bar',
}: {
  sessionId: string;
  send: (message: ClientMessage) => void;
  /** `panel` and `row` are the phone dock's: large keys, wrapping or in its compact row. */
  layout?: 'bar' | 'panel' | 'row';
}) {
  const config = useStore((state) => state.config);
  const prefixActive = useStore((state) => state.prefixActive);
  // The editor covering the active pane, which a double Esc hides on a keyboard.
  const editor = useStore((state) => {
    const paneId = state.getActivePaneId(sessionId);
    if (!paneId) return null;
    if (state.browserOpen && state.browserPaneId === paneId) return 'Browser';
    if (state.vscodeOpen && state.vscodePaneId === paneId) return 'VS Code';
    if (state.nvimOpen && state.nvimPaneId === paneId) return 'Neovim';
    if (nativeEditorOver(state, paneId)) return 'Editor';
    return null;
  });
  const [ctrl, setCtrl] = useState(false);
  const [alt, setAlt] = useState(false);
  const prefix = parsePrefix(getPrefix(config)) ?? parsePrefix(CONFIG_DEFAULTS.prefix)!;

  // To the focused terminal or Neovim, else what covers the active pane.
  const pressKey = (key: string, modifiers: KeyModifiers) => {
    const state = useStore.getState();
    const focused = document.activeElement;
    // Neovim, a pane widget, or a text field such as pane search gets the key itself.
    if (
      focused instanceof HTMLElement &&
      (focused.dataset.nvimInput !== undefined ||
        focused.dataset.browserInput !== undefined ||
        focused.dataset.nativeEditor !== undefined ||
        focused.closest('[data-pane-widget]') ||
        (focused.matches(TEXT_FIELDS) && !focused.closest('[data-pane-terminal]')))
    ) {
      return dispatchKey(focused, key, modifiers);
    }
    const pane = focused?.closest('[data-pane-terminal]');
    for (const term of state.terminals.values()) if (term.element === pane) return term.pressKey(key, modifiers);
    if (state.popupShown) return state.terminals.get(state.popupShown.id)?.pressKey(key, modifiers);
    const paneId = state.getActivePaneId(sessionId);
    if (!paneId) return;
    if (nativeEditorOver(state, paneId)) {
      const input = nativeEditorInput(paneId);
      if (input) dispatchKey(input, key, modifiers);
      return;
    }
    if (state.nvimOpen && state.nvimPaneId === paneId) {
      const input = document.querySelector('[data-nvim-input]');
      if (input) dispatchKey(input, key, modifiers);
      return;
    }
    if (state.browserOpen && state.browserPaneId === paneId) {
      const input = document.querySelector('[data-browser-input]');
      if (input) dispatchKey(input, key, modifiers);
      return;
    }
    // VS Code's iframe ignores synthetic keys; they mustn't reach the shell under it.
    if (state.vscodeOpen && state.vscodePaneId === paneId) return;
    state.terminals.get(paneId)?.pressKey(key, modifiers);
  };

  const press = (key: string) => {
    pressKey(key, { ctrlKey: ctrl, altKey: alt });
    setCtrl(false);
    setAlt(false);
  };

  // While armed, the next character typed on the keyboard takes the modifiers, or the prefix's binding.
  useEffect(() => {
    if (!ctrl && !alt && !prefixActive) return;
    let armed = true;
    const take = (event: Event, key: string) => {
      event.preventDefault();
      event.stopImmediatePropagation();
      armed = false;
      press(key);
    };
    // One character takes them; longer text (a suggestion) goes through and disarms them.
    const typed = (event: Event, text: string | null): text is string => {
      if (!armed || !text || !takesKeys(event.target)) return false;
      if ([...text].length === 1) return true;
      armed = false;
      setCtrl(false);
      setAlt(false);
      useStore.getState().setPrefixActive(false);
      return false;
    };
    const onKeyDown = (event: KeyboardEvent) => {
      // Keys typed after the prefix reach useKeybindings themselves.
      if (!(ctrl || alt) || !event.isTrusted || event.ctrlKey || event.altKey || event.metaKey || event.isComposing) {
        return;
      }
      if ([...event.key].length === 1 && typed(event, event.key)) take(event, event.key);
    };
    // Android keyboards send text without key events, or compose it.
    const onBeforeInput = (event: InputEvent) => {
      if (event.inputType === 'insertText' && typed(event, event.data)) take(event, event.data);
    };
    const onCompositionEnd = (event: CompositionEvent) => {
      if (!typed(event, event.data)) return;
      // End the composition without its text, which goes as the key instead.
      const input = event.target as HTMLTextAreaElement;
      input.value = '';
      input.dispatchEvent(new CompositionEvent('compositionend', { bubbles: true }));
      take(event, event.data);
    };
    window.addEventListener('keydown', onKeyDown, true);
    window.addEventListener('beforeinput', onBeforeInput, true);
    window.addEventListener('compositionend', onCompositionEnd, true);
    return () => {
      window.removeEventListener('keydown', onKeyDown, true);
      window.removeEventListener('beforeinput', onBeforeInput, true);
      window.removeEventListener('compositionend', onCompositionEnd, true);
    };
  }, [ctrl, alt, prefixActive]);

  // The phone layout's floating panel: large keys, wrapping; it hides the keyboard elsewhere.
  const panel = layout !== 'bar';
  const size = panel ? 'lg' : 'sm';
  const keys = (
    <>
      <Button label="Esc" size={size} variant="ghost" onClick={() => press('Escape')} />
      <Button label="Tab" size={size} variant="ghost" onClick={() => press('Tab')} />
      <ToggleButton label="Ctrl" size={size} isPressed={ctrl} onPressedChange={setCtrl} />
      <ToggleButton label="Alt" size={size} isPressed={alt} onPressedChange={setAlt} />
      {ARROWS.map(({ key, label, icon: Icon }) => (
        <IconButton key={key} label={label} size={size} variant="ghost" icon={<Icon />} onClick={() => press(key)} />
      ))}
      <ToggleButton
        label={`Prefix (${getPrefix(config)})`}
        size={size}
        isPressed={prefixActive}
        onPressedChange={(pressed) =>
          pressed
            ? pressKey(prefix.key, { ctrlKey: prefix.ctrl, altKey: prefix.alt })
            : useStore.getState().setPrefixActive(false)
        }
      >
        {getPrefix(config)}
      </ToggleButton>
      {editor && (
        <IconButton
          label={`Hide ${editor}`}
          size={size}
          variant="ghost"
          icon={<PanelBottomClose />}
          onClick={() => {
            const state = useStore.getState();
            const paneId = state.getActivePaneId(sessionId);
            if (editor === 'Browser') state.setBrowserOpen(false);
            else if (editor === 'VS Code') state.setVscodeOpen(false);
            else if (editor === 'Neovim') state.setNvimOpen(false);
            else if (paneId) send({ type: 'editor_view', pane_id: paneId, op: { kind: 'hide' } });
          }}
        />
      )}
    </>
  );
  // Keep the keyboard open: taps here mustn't take focus from the terminal.
  // Not on pointerdown, which cancels a tap's click in WebKit.
  const keepFocus = (event: React.MouseEvent) => event.preventDefault();

  if (panel) {
    return (
      <HStack
        gap={layout === 'row' ? 0.5 : 1}
        padding={layout === 'row' ? 0 : 1}
        vAlign="center"
        // In the dock's compact row, whose 40px squares would clip Esc and Ctrl.
        className={`btm-mono ${layout === 'row' ? 'shrink-0 [&_button]:h-10 [&_button]:min-w-10 [&_button]:w-auto! [&_button]:px-2!' : 'flex-wrap [&_button]:h-11 [&_button]:min-w-11'}`}
        aria-label="Terminal keys"
        onMouseDown={keepFocus}
      >
        {keys}
      </HStack>
    );
  }
  return (
    <HStack
      gap={1}
      paddingInline={1}
      paddingBlock={1}
      vAlign="center"
      className="btm-mono"
      aria-label="Terminal keys"
      onMouseDown={keepFocus}
    >
      {/* Scroll sideways rather than truncate the keys. */}
      <HStack
        vAlign="center"
        isScrollable
        // Tight keys, so a typical phone fits them all without scrolling.
        className="min-w-0 flex-1 [&>*]:shrink-0 [&_button]:px-1"
      >
        {keys}
      </HStack>
      {/* iPhone keyboards have no dismiss key. */}
      <IconButton label="Hide keyboard" size="sm" variant="ghost" icon={<KeyboardOff />} onClick={hideKeyboard} />
    </HStack>
  );
}

import { useEffect, useSyncExternalStore, type KeyboardEvent } from 'react';
import { IconButton } from '@astryxdesign/core/IconButton';
import { HStack, VStack } from '@astryxdesign/core/Layout';
import { TextInput } from '@astryxdesign/core/TextInput';
import { ToggleButton } from '@astryxdesign/core/ToggleButton';
import { ArrowDown, ArrowUp, CaseSensitive, Regex, Replace, ReplaceAll, TextSelect, WholeWord, X } from 'lucide-react';
import type { SearchPanelHandle } from '../lib/nativeEditor/searchPanel';
import { IS_MAC } from '../lib/nvimKeys';

const MOD = IS_MAC ? 'Cmd' : 'Ctrl';

/** The native editor's find and replace panel, in CodeMirror's panel (`searchPanel`). */
export function EditorSearch({ handle }: { handle: SearchPanelHandle }) {
  const query = useSyncExternalStore(handle.subscribe, handle.query);

  // CodeMirror focuses the `main-field` input when the panel is opened again.
  useEffect(() => {
    const input = handle.dom.querySelector<HTMLInputElement>('input[name=search]');
    input?.setAttribute('main-field', 'true');
    input?.focus();
    input?.select();
  }, [handle]);

  // The editor's panel keys first; Enter steps through matches or replaces,
  // and with Alt (Mod-Alt) selects or replaces them all.
  const keyDown = (field: 'search' | 'replace') => (e: KeyboardEvent<HTMLInputElement>) => {
    if (handle.key(e.nativeEvent)) {
      e.preventDefault();
      return;
    }
    if (e.key !== 'Enter' || e.nativeEvent.isComposing) return;
    e.preventDefault();
    if (field === 'search') {
      if (e.altKey) handle.selectAll();
      else if (e.shiftKey) handle.previous();
      else handle.next();
    } else if (e.altKey && (e.metaKey || e.ctrlKey)) handle.replaceAll();
    else handle.replace();
  };

  const invalid = query.search !== '' && !query.valid;
  return (
    <VStack gap={1} className="btm-sans p-1.5">
      <HStack gap={1} vAlign="center">
        <TextInput
          label="Find"
          isLabelHidden
          placeholder="Find"
          size="sm"
          htmlName="search"
          value={query.search}
          onChange={(search) => handle.set({ search })}
          onKeyDown={keyDown('search')}
          status={invalid ? { type: 'error', message: 'Not a valid regular expression' } : undefined}
          statusVariant="tooltip"
          containerClassName="min-w-0 flex-1"
          controlClassName="btm-mono"
        />
        <ToggleButton
          label="Match case"
          icon={<CaseSensitive />}
          isIconOnly
          size="sm"
          isPressed={query.caseSensitive}
          onPressedChange={(caseSensitive) => handle.set({ caseSensitive })}
        />
        <ToggleButton
          label="Match whole word"
          icon={<WholeWord />}
          isIconOnly
          size="sm"
          isPressed={query.wholeWord}
          onPressedChange={(wholeWord) => handle.set({ wholeWord })}
        />
        <ToggleButton
          label="Use regular expression"
          icon={<Regex />}
          isIconOnly
          size="sm"
          isPressed={query.regexp}
          onPressedChange={(regexp) => handle.set({ regexp })}
        />
        <IconButton
          label="Previous match"
          tooltip="Previous match (Shift+Enter)"
          icon={<ArrowUp />}
          size="sm"
          variant="ghost"
          onClick={handle.previous}
        />
        <IconButton
          label="Next match"
          tooltip="Next match (Enter)"
          icon={<ArrowDown />}
          size="sm"
          variant="ghost"
          onClick={handle.next}
        />
        <IconButton
          label="Select all matches"
          tooltip="Select all matches (Alt+Enter)"
          icon={<TextSelect />}
          size="sm"
          variant="ghost"
          onClick={handle.selectAll}
        />
        <IconButton label="Close" tooltip="Close (Esc)" icon={<X />} size="sm" variant="ghost" onClick={handle.close} />
      </HStack>
      <HStack gap={1} vAlign="center">
        <TextInput
          label="Replace"
          isLabelHidden
          placeholder="Replace"
          size="sm"
          htmlName="replace"
          value={query.replace}
          onChange={(replace) => handle.set({ replace })}
          onKeyDown={keyDown('replace')}
          containerClassName="min-w-0 flex-1"
          controlClassName="btm-mono"
        />
        <IconButton
          label="Replace"
          tooltip="Replace (Enter)"
          icon={<Replace />}
          size="sm"
          variant="ghost"
          onClick={handle.replace}
        />
        <IconButton
          label="Replace all"
          tooltip={`Replace all (${MOD}+Alt+Enter)`}
          icon={<ReplaceAll />}
          size="sm"
          variant="ghost"
          onClick={handle.replaceAll}
        />
      </HStack>
    </VStack>
  );
}

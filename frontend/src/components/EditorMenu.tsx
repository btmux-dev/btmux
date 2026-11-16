import { useState } from 'react';
import { DropdownMenu } from '@astryxdesign/core/DropdownMenu';
import { IconButton } from '@astryxdesign/core/IconButton';
import type { Editor, EditorsStatus } from '../generated/protocol';
import { useStore } from '../state/store';
import { editorButton } from './EditorIcon';

/** Status bar menu of a built-in editor: show or hide it, or shut its server down. */
export function EditorMenu({
  editor,
  paneId,
  onToggle,
  onShutDown,
}: {
  editor: Exclude<Editor, 'pane'>;
  /** The active pane, which the surface toggles over. */
  paneId: string | undefined;
  onToggle: () => void;
  onShutDown: () => void;
}) {
  const button = editorButton(editor);
  // Unknown until the menu has asked.
  const [running, setRunning] = useState<boolean | null>(null);
  const shown = useStore((s) =>
    editor === 'neovim' ? s.nvimOpen && s.nvimPaneId === paneId : s.vscodeOpen && s.vscodePaneId === paneId,
  );
  const refresh = () => {
    setRunning(null);
    fetch('/api/editors')
      .then((r) => (r.ok ? (r.json() as Promise<EditorsStatus>) : Promise.reject(new Error(r.statusText))))
      .then((status) => setRunning(status.running.includes(editor)))
      .catch(() => setRunning(true));
  };
  return (
    <DropdownMenu
      placement="above"
      alignment="end"
      presentation="adaptive"
      hasChevron={false}
      onOpenChange={(open) => open && refresh()}
      renderTrigger={(props) => (
        <IconButton
          label={button.label}
          tooltip={button.label}
          variant="ghost"
          size="sm"
          icon={button.icon}
          {...props}
        />
      )}
      items={[
        { label: shown ? `Hide ${button.label}` : `Show ${button.label}`, onClick: onToggle },
        {
          label: 'Shut down',
          description: running === false ? 'Not running' : undefined,
          variant: 'destructive',
          isDisabled: running !== true,
          onClick: onShutDown,
        },
      ]}
    />
  );
}

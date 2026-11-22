import { useLayoutEffect, useRef, useState } from 'react';
import { Dialog, DialogHeader } from '@astryxdesign/core/Dialog';
import { Heading } from '@astryxdesign/core/Heading';
import { Text } from '@astryxdesign/core/Text';
import { List, ListItem } from '@astryxdesign/core/List';
import { Layout, LayoutContent, LayoutHeader, HStack, VStack, Section } from '@astryxdesign/core/Layout';
import { TabList, Tab } from '@astryxdesign/core/TabList';
import type { Bind, ClientConfig } from '../state/types';
import { getPrefix, getSmoothScrollDuration, getVscodeKeys } from '../state/configDefaults';
import { KeyCap } from './KeyHint';
import { EditorKeys } from './EditorKeys';
import { actionLabel } from '../lib/actionLabel';
import { EDITOR_COMMANDS } from '../lib/nativeEditor/registry';
import { cmKeyCap, editorKeyRows } from '../lib/nativeEditor/keyHelp';
import { OPEN_SETTINGS, workbenchKeys } from '../lib/workbenchKeys';
import { smoothScrollBy } from '../lib/smoothScroll';
import { cycleTabs, enterTabPanel } from '../lib/tabKeys';

/** Ordered keybinding-help sections, each matching a set of action names. */
export const KEY_SECTIONS: { title: string; actions: string[] }[] = [
  {
    title: 'Panes',
    actions: [
      'split-horizontal',
      'split-vertical',
      'navigate-left',
      'navigate-right',
      'navigate-up',
      'navigate-down',
      'resize-pane-left',
      'resize-pane-right',
      'resize-pane-up',
      'resize-pane-down',
      'resize-pane-left-5',
      'resize-pane-right-5',
      'resize-pane-up-5',
      'resize-pane-down-5',
      'zoom-pane',
      'kill-pane',
      'next-pane',
      'last-pane',
      'swap-pane-back',
      'swap-pane-forward',
      'next-layout',
      'display-panes',
      'capture-pane',
      'search-pane',
      'scroll-page-up',
      'scroll-page-down',
      'paste-clipboard',
      'toggle-latex',
      'file-browser',
      'git-view',
      'process-view',
      'editor',
      'editor-native',
      ...EDITOR_COMMANDS,
      'editor-neovim',
      'editor-vscode',
      'shut-down-neovim',
      'shut-down-vscode',
      'browser',
    ],
  },
  {
    title: 'Windows',
    actions: [
      'new-window',
      'next-window',
      'prev-window',
      'last-window',
      'rename-window',
      'window-grid',
      'agent-grid',
      'kill-window',
      'new-worktree',
      'remove-worktree',
    ],
  },
  {
    title: 'Sessions',
    actions: [
      'choose-session',
      'new-session',
      'rename-session',
      'kill-session',
      'next-session',
      'prev-session',
      'last-session',
    ],
  },
  {
    title: 'General',
    actions: [
      'command-palette',
      'list-keys',
      'toggle-key-overlay',
      'choose-appearance',
      'choose-colors',
      'choose-font',
      'choose-font-weight',
    ],
  },
];

/** Unique actions of a keybinding-help section, each with all of its keys. */
function sectionBinds(actions: string[], binds: Bind[]): { action: string; keys: string[] }[] {
  return actions.flatMap((action) => {
    const keys = binds.filter((bind) => bind.action === action).map((bind) => bind.key);
    return keys.length ? [{ action, keys }] : [];
  });
}

type KeysTab = 'prefix' | 'vscode' | 'editor';

const TAB_LABELS: Record<KeysTab, string> = { prefix: 'Prefix', vscode: 'Shortcuts', editor: 'Editor' };

export function KeysDialog({
  title,
  binds,
  config,
  onClose,
}: {
  title: string;
  binds: Bind[];
  config: ClientConfig | null;
  onClose: () => void;
}) {
  const [tab, setTab] = useState<KeysTab>('prefix');
  const keysRef = useRef<HTMLDivElement>(null);
  const tabs: KeysTab[] = [
    'prefix',
    ...(getVscodeKeys(config) ? ['vscode' as const] : []),
    ...(config ? ['editor' as const] : []),
  ];
  const shown = tabs.includes(tab) ? tab : 'prefix';
  const dialogRef = useRef<HTMLDialogElement>(null);
  // A tab the config removed falls back to Prefix, keeping focus in the dialog.
  useLayoutEffect(() => {
    if (shown === tab) return;
    setTab(shown);
    const dialog = dialogRef.current;
    if (dialog && !dialog.contains(document.activeElement)) dialog.querySelector<HTMLElement>('[role="tab"]')?.focus();
  });
  const changeOpen = (open: boolean) => {
    if (!open) onClose();
  };
  const sections = [
    ...KEY_SECTIONS,
    {
      title: 'Other',
      actions: [
        ...new Set(
          binds
            .filter((bind) => !KEY_SECTIONS.some((section) => section.actions.includes(bind.action)))
            .map((bind) => bind.action),
        ),
      ],
    },
  ];
  const panel = (id: KeysTab) => ({ id: `keys-${id}`, role: 'tabpanel', 'aria-label': TAB_LABELS[id] });
  return (
    <Dialog
      ref={dialogRef}
      isOpen
      onOpenChange={changeOpen}
      width={960}
      maxHeight="85dvh"
      // Fill maxHeight so switching tabs doesn't resize the dialog.
      className="h-full"
      // Clicking text keeps focus here, so Tab still reaches the handler.
      tabIndex={-1}
      onKeyDown={(event) => {
        event.stopPropagation();
        if (cycleTabs(event, tabs, shown, setTab) || enterTabPanel(event)) return;
        const direction = !event.ctrlKey ? 0 : event.key === 'd' ? 1 : event.key === 'u' ? -1 : 0;
        const viewport = keysRef.current;
        if (direction && viewport) {
          event.preventDefault();
          smoothScrollBy(viewport, direction * (viewport.clientHeight / 2), getSmoothScrollDuration(config));
        }
      }}
    >
      <Layout
        header={<DialogHeader title={title} hasDivider={false} onOpenChange={changeOpen} />}
        content={
          <Layout
            header={
              <LayoutHeader paddingBlockEnd={0}>
                <TabList
                  value={shown}
                  onChange={(value) => setTab(value as KeysTab)}
                  role="tablist"
                  hasDivider
                  isFullBleed
                >
                  {tabs.map((id) => (
                    <Tab key={id} value={id} label={TAB_LABELS[id]} panelId={`keys-${id}`} />
                  ))}
                </TabList>
              </LayoutHeader>
            }
            content={
              // Keyed so each panel starts at the top.
              <LayoutContent key={shown} ref={keysRef} padding={4}>
                {shown === 'vscode' ? (
                  <VStack gap={2} {...panel('vscode')} data-workbench-key-help>
                    <Text color="secondary">As in VS Code, in the focused pane or popup.</Text>
                    <List density="compact" hasDividers>
                      {workbenchKeys()
                        // Quick open is the native editor's.
                        .filter(({ action }) => action !== 'editor-quick-open' || config?.editor === 'native')
                        .map(({ action, key, terminal }) => (
                          <ListItem
                            key={action}
                            label={
                              (action === OPEN_SETTINGS ? 'Settings' : actionLabel(action, config?.popups)) +
                              (terminal ? ' (in a terminal)' : '')
                            }
                            endContent={<KeyCap keys={cmKeyCap(key)} />}
                          />
                        ))}
                    </List>
                  </VStack>
                ) : shown === 'editor' && config ? (
                  <VStack gap={2} {...panel('editor')} data-editor-key-help>
                    <Text color="secondary">
                      {config.vim_mode
                        ? 'In the native editor. Vim mode is on.'
                        : 'In the native editor. Vim mode is off.'}
                    </Text>
                    <List density="compact" hasDividers>
                      {editorKeyRows(config).map((row, i) => (
                        <ListItem key={i} label={row.label} endContent={<EditorKeys keys={row.keys} />} />
                      ))}
                    </List>
                  </VStack>
                ) : (
                  <VStack gap={0} {...panel('prefix')}>
                    <Text color="secondary">Press {getPrefix(config)}, then a key.</Text>
                    {sections.map((section) => {
                      const rows = sectionBinds(section.actions, binds);
                      return rows.length ? (
                        <Section key={section.title} variant="transparent">
                          <Heading level={3}>{section.title}</Heading>
                          <List density="compact" hasDividers>
                            {rows.map(({ action, keys }) => (
                              <ListItem
                                key={action}
                                label={actionLabel(action, config?.popups)}
                                endContent={
                                  <HStack gap={1}>
                                    {keys.map((key) => (
                                      <KeyCap key={key} keys={key} />
                                    ))}
                                  </HStack>
                                }
                              />
                            ))}
                          </List>
                        </Section>
                      ) : null;
                    })}
                  </VStack>
                )}
              </LayoutContent>
            }
          />
        }
      />
    </Dialog>
  );
}

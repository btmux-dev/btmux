import { useEffect, useRef, useState } from 'react';
import { Dialog, DialogHeader } from '@astryxdesign/core/Dialog';
import { Layout, LayoutContent, LayoutFooter, HStack, VStack } from '@astryxdesign/core/Layout';
import { TextArea } from '@astryxdesign/core/TextArea';
import { CheckboxList, CheckboxListItem } from '@astryxdesign/core/CheckboxList';
import { Button } from '@astryxdesign/core/Button';
import { Text } from '@astryxdesign/core/Text';
import { useStore } from '../state/store';
import type { AgentStatus } from '../state/types';
import { AgentStatusBadge } from './PaneTitleBar';

export interface BroadcastTarget {
  paneId: string;
  label: string;
  /** Session › window. */
  place: string;
  status: AgentStatus;
}

interface Props {
  isOpen: boolean;
  targets: BroadcastTarget[];
  onClose: () => void;
}

/** Submits one prompt to each chosen agent, queued behind any running turn. */
async function sendPrompt(paneId: string, prompt: string): Promise<void> {
  const response = await fetch(`/api/panes/${encodeURIComponent(paneId)}/prompt`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ prompt, wait: false }),
  });
  if (!response.ok) throw new Error((await response.text()) || response.statusText);
}

/** A prompt typed once and sent to several agents. */
export function AgentBroadcastDialog({ isOpen, targets, onClose }: Props) {
  const [prompt, setPrompt] = useState('');
  const [chosen, setChosen] = useState<string[]>([]);
  const [sending, setSending] = useState(false);
  const promptRef = useRef<HTMLTextAreaElement>(null);
  const wasOpen = useRef(false);

  // Each opening starts with every agent not waiting on a dialog, which typed text would answer.
  useEffect(() => {
    if (isOpen && !wasOpen.current) {
      setChosen(targets.filter((t) => t.status.state !== 'blocked').map((t) => t.paneId));
      requestAnimationFrame(() => promptRef.current?.focus());
    }
    wasOpen.current = isOpen;
  }, [isOpen, targets]);

  const live = new Set(targets.map((t) => t.paneId));
  const recipients = chosen.filter((id) => live.has(id));
  const canSend = !sending && prompt.trim() !== '' && recipients.length > 0;

  const submit = async () => {
    if (!canSend) return;
    setSending(true);
    const text = prompt;
    const results = await Promise.allSettled(recipients.map((id) => sendPrompt(id, text)));
    setSending(false);
    const { showToast } = useStore.getState();
    const failures = results.flatMap((result, i) => {
      if (result.status === 'fulfilled') return [];
      const target = targets.find((t) => t.paneId === recipients[i]);
      const reason = result.reason instanceof Error ? result.reason.message : String(result.reason);
      return [`${target ? `${target.place} › ${target.label}` : recipients[i]}: ${reason}`];
    });
    const sent = recipients.length - failures.length;
    if (failures.length === 0) {
      showToast(`Sent to ${sent} agent${sent === 1 ? '' : 's'}`, 'success');
      setPrompt('');
      onClose();
    } else {
      showToast(`Sent to ${sent} of ${recipients.length} agents`, 'error', { body: failures.join('\n') });
      // Keeps the prompt and only the agents that missed it, to retry.
      setChosen(recipients.filter((_, i) => results[i].status === 'rejected'));
    }
  };

  const allChosen = recipients.length === targets.length;

  return (
    <Dialog
      isOpen={isOpen}
      onOpenChange={(value) => {
        if (!value) onClose();
      }}
      purpose="form"
      width={560}
      onKeyDown={(e: React.KeyboardEvent) => {
        // React bubbles portal events to the grid's handler.
        e.stopPropagation();
        if (e.key === 'Escape') {
          e.preventDefault();
          onClose();
        } else if (
          e.key === 'Enter' &&
          !e.shiftKey &&
          !e.nativeEvent.isComposing &&
          !(e.target as Element).closest('button')
        ) {
          // Like an agent's own prompt; Cmd chords never reach dialogs. A focused button activates itself.
          e.preventDefault();
          void submit();
        }
      }}
    >
      <Layout
        header={<DialogHeader title="Broadcast prompt" onOpenChange={(value) => !value && onClose()} />}
        content={
          <LayoutContent padding={4}>
            <VStack gap={4}>
              <TextArea
                ref={promptRef}
                label="Prompt"
                value={prompt}
                onChange={setPrompt}
                rows={5}
                placeholder="Sent to each agent below, queued if it is busy"
              />
              <VStack gap={1}>
                <HStack gap={2} vAlign="center" hAlign="between">
                  <Text type="label">Agents</Text>
                  <Button
                    label={allChosen ? 'Select none' : 'Select all'}
                    variant="ghost"
                    size="sm"
                    onClick={() => setChosen(allChosen ? [] : targets.map((t) => t.paneId))}
                  />
                </HStack>
                <CheckboxList label="Agents" isLabelHidden density="compact" value={recipients} onChange={setChosen}>
                  {targets.map((target) => (
                    <CheckboxListItem
                      key={target.paneId}
                      value={target.paneId}
                      label={target.label}
                      description={<Text className="btm-mono">{target.place}</Text>}
                      endContent={<AgentStatusBadge status={target.status} />}
                    />
                  ))}
                </CheckboxList>
              </VStack>
            </VStack>
          </LayoutContent>
        }
        footer={
          <LayoutFooter hasDivider>
            <HStack gap={2} vAlign="center" hAlign="end">
              <Text type="supporting" color="secondary">
                Shift+Enter for a new line
              </Text>
              <Button label="Cancel" variant="ghost" onClick={onClose} />
              <Button
                label={recipients.length ? `Send to ${recipients.length}` : 'Send'}
                variant="primary"
                isDisabled={!canSend}
                isLoading={sending}
                onClick={() => void submit()}
              />
            </HStack>
          </LayoutFooter>
        }
      />
    </Dialog>
  );
}

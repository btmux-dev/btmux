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
  agentId: number | null;
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

/** One agent instance: a new agent in the same pane is another target, never checked for it. */
const keyOf = (target: BroadcastTarget) => `${target.paneId}:${target.agentId}`;

/** Typed text would answer a blocked agent's question, so the server refuses it. */
const eligible = (target: BroadcastTarget) => target.agentId != null && target.status.state !== 'blocked';

/** `failed`: nothing reached the agent. `unknown`: it may have; resending could repeat it. */
type Outcome = { kind: 'sent' } | { kind: 'failed' | 'unknown'; reason: string };

async function sendPrompt(target: BroadcastTarget, prompt: string): Promise<Outcome> {
  let response: Response;
  try {
    response = await fetch(`/api/panes/${encodeURIComponent(target.paneId)}/prompt`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ prompt, wait: false, agent_id: target.agentId }),
    });
  } catch (error) {
    // The server may have submitted it before the reply was lost.
    return { kind: 'unknown', reason: error instanceof Error ? error.message : String(error) };
  }
  if (response.ok) return { kind: 'sent' };
  const reason = (await response.text().catch(() => '')) || response.statusText;
  // 4xx are refusals; a 409 after pasting (automation::send_prompt_to) and 5xx may have left text in the pane.
  const unknown = response.status >= 500 || reason.includes('was pasted');
  return { kind: unknown ? 'unknown' : 'failed', reason };
}

/** A prompt typed once and sent to several agents. */
export function AgentBroadcastDialog({ isOpen, targets, onClose }: Props) {
  const [prompt, setPrompt] = useState('');
  // The draft's recipients, kept with it; null picks every eligible agent at the next opening.
  const [chosen, setChosen] = useState<string[] | null>(null);
  // Agents that may have received the draft already, unchecked until the user checks them again.
  const [unknown, setUnknown] = useState<ReadonlySet<string>>(new Set());
  const [sending, setSending] = useState(false);
  const promptRef = useRef<HTMLTextAreaElement>(null);
  const wasOpen = useRef(false);
  const openRef = useRef(isOpen);
  openRef.current = isOpen;

  useEffect(() => {
    if (isOpen && !wasOpen.current) {
      if (chosen === null) setChosen(targets.filter(eligible).map(keyOf));
      requestAnimationFrame(() => promptRef.current?.focus());
    }
    wasOpen.current = isOpen;
  }, [isOpen, targets, chosen]);

  const byKey = new Map(targets.map((t) => [keyOf(t), t]));
  const recipients = (chosen ?? []).flatMap((key) => {
    const target = byKey.get(key);
    return target && eligible(target) ? [target] : [];
  });
  const canSend = !sending && prompt.trim() !== '' && recipients.length > 0;
  const eligibleKeys = targets.filter(eligible).map(keyOf);
  const allChosen = eligibleKeys.length > 0 && eligibleKeys.every((key) => chosen?.includes(key));

  const submit = async () => {
    if (!canSend) return;
    // The draft and its recipients stay read-only until every reply is in, even if the dialog is closed meanwhile.
    setSending(true);
    const outcomes = await Promise.all(recipients.map((target) => sendPrompt(target, prompt)));
    setSending(false);
    const describe = (target: BroadcastTarget, reason: string) => `${target.place} › ${target.label}: ${reason}`;
    const failed: string[] = [];
    const uncertain: string[] = [];
    const lines: string[] = [];
    outcomes.forEach((outcome, i) => {
      if (outcome.kind === 'sent') return;
      (outcome.kind === 'failed' ? failed : uncertain).push(keyOf(recipients[i]));
      const note = outcome.kind === 'unknown' ? ' (may have received it)' : '';
      lines.push(describe(recipients[i], outcome.reason + note));
    });
    const sent = recipients.length - failed.length - uncertain.length;
    const { showToast } = useStore.getState();
    if (lines.length === 0) {
      showToast(`Sent to ${sent} agent${sent === 1 ? '' : 's'}`, 'success');
      setPrompt('');
      setChosen(null);
      setUnknown(new Set());
      if (openRef.current) onClose();
      return;
    }
    showToast(`Sent to ${sent} of ${recipients.length} agents`, 'error', { body: lines.join('\n') });
    // Only agents that certainly didn't get it are checked again; the rest need a deliberate check.
    setChosen(failed);
    setUnknown(new Set(uncertain));
  };

  const description = (target: BroadcastTarget) => {
    const note =
      target.status.state === 'blocked'
        ? 'Waiting for a decision; answer it in its pane first'
        : unknown.has(keyOf(target))
          ? 'May have received this prompt already'
          : null;
    return (
      <VStack gap={0.5}>
        <Text className="btm-mono">{target.place}</Text>
        {note && <Text color="secondary">{note}</Text>}
      </VStack>
    );
  };

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
                isReadOnly={sending}
                rows={5}
                placeholder="Sent to each agent checked below"
              />
              <VStack gap={1}>
                <HStack gap={2} vAlign="center" hAlign="between">
                  <Text type="label">Agents</Text>
                  <Button
                    label={allChosen ? 'Select none' : 'Select all'}
                    variant="ghost"
                    size="sm"
                    isDisabled={sending || eligibleKeys.length === 0}
                    onClick={() => setChosen(allChosen ? [] : eligibleKeys)}
                  />
                </HStack>
                <CheckboxList
                  label="Agents"
                  isLabelHidden
                  density="compact"
                  isReadOnly={sending}
                  value={recipients.map(keyOf)}
                  onChange={setChosen}
                >
                  {targets.map((target) => (
                    <CheckboxListItem
                      key={keyOf(target)}
                      value={keyOf(target)}
                      label={target.label}
                      isDisabled={!eligible(target)}
                      description={description(target)}
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
              <Button label="Close" variant="ghost" onClick={onClose} />
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

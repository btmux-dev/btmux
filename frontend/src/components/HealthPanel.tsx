import { useEffect, useState } from 'react';
import type { HealthCheck, HealthEvent, HealthStatus } from '../generated/protocol';
import { Banner } from '@astryxdesign/core/Banner';
import { HStack, VStack } from '@astryxdesign/core/Layout';
import { MetadataList, MetadataListItem } from '@astryxdesign/core/MetadataList';
import { StatusDot } from '@astryxdesign/core/StatusDot';
import { Text } from '@astryxdesign/core/Text';
import { useStore } from '../state/store';
import { browserHealth } from '../lib/browserHealth';
import { Spinner } from './Spinner';

const LABEL = { position: 'start', width: 152 } as const;

const DOT = {
  ok: { variant: 'success', label: 'OK' },
  missing: { variant: 'neutral', label: 'Not available' },
  warning: { variant: 'warning', label: 'Warning' },
  error: { variant: 'error', label: 'Error' },
} as const satisfies Record<HealthStatus, { variant: string; label: string }>;

type Group = { title: string; names: string[]; checks: (HealthCheck | null)[] };
type Report = { groups: Group[]; path: string[] };

/** `/api/health/stream`'s events, as each check finishes. */
async function streamHealth(signal: AbortSignal, onEvent: (event: HealthEvent) => void) {
  const response = await fetch('/api/health/stream', { signal, cache: 'no-store' });
  if (!response.ok || !response.body) throw new Error(`Could not check the server (${response.status}).`);
  const reader = response.body.pipeThrough(new TextDecoderStream()).getReader();
  let buffered = '';
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    buffered += value;
    const lines = buffered.split('\n');
    buffered = lines.pop() ?? '';
    for (const line of lines) if (line.trim()) onEvent(JSON.parse(line) as HealthEvent);
  }
}

function applyEvent(report: Report | null, event: HealthEvent): Report | null {
  if (event.type === 'plan')
    return {
      groups: event.groups.map((group) => ({
        title: group.title,
        names: group.checks,
        checks: group.checks.map(() => null),
      })),
      path: event.path,
    };
  if (!report) return report;
  const groups = report.groups.map((group, index) =>
    index === event.group
      ? { ...group, checks: group.checks.map((check, i) => (i === event.index ? event.check : check)) }
      : group,
  );
  return { ...report, groups };
}

/** Which optional programs the server found, and what this browser supports. */
export function HealthPanel() {
  const [report, setReport] = useState<Report | null>(null);
  const [ended, setEnded] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [browser, setBrowser] = useState<HealthCheck[] | null>(null);
  const osNotifications = useStore((s) => s.config?.notifications.os ?? true);

  useEffect(() => {
    const controller = new AbortController();
    streamHealth(controller.signal, (event) => setReport((report) => applyEvent(report, event)))
      .then(() => setEnded(true))
      .catch((error: unknown) => {
        if (controller.signal.aborted) return;
        setEnded(true);
        setError(error instanceof Error ? error.message : 'Could not check the server.');
      });
    return () => controller.abort();
  }, []);

  useEffect(() => {
    let current = true;
    browserHealth(osNotifications).then((checks) => current && setBrowser(checks));
    return () => {
      current = false;
    };
  }, [osNotifications]);

  const server = report?.groups.flatMap((group) => group.checks) ?? [];
  const finished = report !== null && server.every((check) => check !== null);
  const failed = error ?? (ended && !finished ? 'The server stopped before finishing its checks.' : null);
  const checks = [...server.filter((check) => check !== null), ...(browser ?? [])];
  return (
    <VStack gap={6} className="select-text">
      {failed ? (
        <Banner status="error" title={failed} />
      ) : !finished ? (
        <HStack gap={2} vAlign="center">
          <Spinner size="sm" aria-label="Checking the server" />
          <Text>Checking the server…</Text>
        </HStack>
      ) : (
        browser && <Summary checks={checks} />
      )}
      {report?.groups.map((group) => (
        <Group key={group.title} group={group} ended={ended} />
      ))}
      {browser ? (
        <Group group={{ title: 'This browser', names: browser.map((check) => check.name), checks: browser }} />
      ) : (
        <HStack gap={2} vAlign="center">
          <Spinner size="sm" aria-label="Checking this browser" />
          <Text>Checking this browser…</Text>
        </HStack>
      )}
      {report && (
        <MetadataList title={<Text type="large">Search path</Text>} label={{ position: 'top' }}>
          <MetadataListItem label="btmux looks for programs on the server's PATH, which can differ from your shell's.">
            <VStack gap={1}>
              {report.path.map((dir, index) => (
                <Text key={`${index} ${dir}`} type="code" className="break-all">
                  {dir}
                </Text>
              ))}
            </VStack>
          </MetadataListItem>
        </MetadataList>
      )}
    </VStack>
  );
}

function Summary({ checks }: { checks: HealthCheck[] }) {
  const problems = checks.filter((check) => check.status === 'error' || check.status === 'warning');
  if (!problems.length) return <Banner status="success" title="No problems found" />;
  return (
    <Banner
      status={problems.some((check) => check.status === 'error') ? 'error' : 'warning'}
      title={`Needs attention: ${problems.map((check) => check.name).join(', ')}`}
    />
  );
}

function Group({ group, ended = false }: { group: Group; ended?: boolean }) {
  return (
    <MetadataList title={<Text type="large">{group.title}</Text>} label={LABEL}>
      {group.names.map((name, index) => {
        const check = group.checks[index];
        if (!check)
          return (
            <MetadataListItem key={index} label={name}>
              {ended ? (
                <Text color="secondary">Not checked</Text>
              ) : (
                <HStack gap={2} vAlign="center" data-health-checking>
                  <Spinner size="sm" aria-label={`Checking ${name}`} />
                  <Text color="secondary">Checking…</Text>
                </HStack>
              )}
            </MetadataListItem>
          );
        return (
          <MetadataListItem key={index} label={check.name}>
            <VStack gap={1}>
              <HStack gap={2} vAlign="center">
                <StatusDot {...DOT[check.status]} />
                <Text type="code" className="break-all">
                  {check.value}
                </Text>
              </HStack>
              {check.note && <Text>{check.note}</Text>}
              <Text color="secondary">{check.purpose}</Text>
              {check.path && (
                <Text type="code" color="secondary" className="break-all">
                  {check.path}
                </Text>
              )}
            </VStack>
          </MetadataListItem>
        );
      })}
    </MetadataList>
  );
}

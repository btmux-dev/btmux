import type { AgentState } from '../state/types';

export type AgentGroup = 'blocked' | 'done' | 'working' | 'waiting' | 'idle';

/** Most urgent first: waiting on you, then results to read, then the rest. */
export const AGENT_GROUPS: readonly AgentGroup[] = ['blocked', 'done', 'working', 'waiting', 'idle'];

export const AGENT_GROUP_LABELS: Record<AgentGroup, string> = {
  blocked: 'Blocked',
  done: 'Done',
  working: 'Working',
  waiting: 'Waiting',
  idle: 'Idle',
};

export const agentGroup = (state: AgentState): AgentGroup => (state === 'unknown' ? 'idle' : state);

/**
 * Groups in urgency order; within one, the longest in its state first, so
 * agents keep their place until their state changes. Ties keep input order.
 */
export function groupAgents<T>(
  items: readonly T[],
  state: (item: T) => AgentState,
  since: (item: T) => number | null,
): [AgentGroup, T[]][] {
  return AGENT_GROUPS.map((group) => {
    const members = items.filter((item) => agentGroup(state(item)) === group);
    // `sort` is stable.
    members.sort((a, b) => (since(a) ?? Infinity) - (since(b) ?? Infinity));
    return [group, members] as [AgentGroup, T[]];
  }).filter(([, members]) => members.length > 0);
}

/** How long an agent has been in its state, from Unix seconds. */
export function formatSince(since: number, nowSeconds: number): string {
  const seconds = Math.max(0, Math.floor(nowSeconds - since));
  const minutes = Math.floor(seconds / 60);
  const hours = Math.floor(minutes / 60);
  const days = Math.floor(hours / 24);
  if (minutes < 1) return '<1m';
  if (hours < 1) return `${minutes}m`;
  if (days < 1) return `${hours}h ${minutes % 60}m`;
  return `${days}d ${hours % 24}h`;
}

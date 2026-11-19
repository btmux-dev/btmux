import { describe, expect, test } from 'bun:test';
import { formatSince, groupAgents } from './src/lib/agentGroups';
import type { AgentState } from './src/state/types';

describe('groupAgents', () => {
  const agents: { id: string; state: AgentState; since: number | null }[] = [
    { id: 'a', state: 'working', since: 30 },
    { id: 'b', state: 'idle', since: 10 },
    { id: 'c', state: 'blocked', since: 50 },
    { id: 'd', state: 'unknown', since: 5 },
    { id: 'e', state: 'blocked', since: 20 },
    { id: 'f', state: 'working', since: 30 },
    { id: 'g', state: 'done', since: null },
    { id: 'h', state: 'waiting', since: 40 },
  ];
  const grouped = groupAgents(
    agents,
    (agent) => agent.state,
    (agent) => agent.since,
  ).map(([group, members]) => [group, members.map((agent) => agent.id)]);

  test('orders groups by urgency and drops empty ones', () => {
    expect(grouped.map(([group]) => group)).toEqual(['blocked', 'done', 'working', 'waiting', 'idle']);
    expect(
      groupAgents(
        [],
        () => 'idle',
        () => null,
      ),
    ).toEqual([]);
  });

  test('puts the longest waiting first, keeping input order on ties', () => {
    expect(grouped).toEqual([
      ['blocked', ['e', 'c']],
      ['done', ['g']],
      ['working', ['a', 'f']],
      ['waiting', ['h']],
      ['idle', ['d', 'b']],
    ]);
  });
});

describe('formatSince', () => {
  test('rounds down to the largest units', () => {
    expect(formatSince(100, 130)).toBe('<1m');
    expect(formatSince(100, 100 + 59 * 60 + 59)).toBe('59m');
    expect(formatSince(0, 3 * 3600 + 7 * 60)).toBe('3h 7m');
    expect(formatSince(0, 2 * 86400 + 5 * 3600)).toBe('2d 5h');
  });

  test('treats a clock behind the server as now', () => {
    expect(formatSince(200, 100)).toBe('<1m');
  });
});

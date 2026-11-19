import { useMemo } from 'react';
import { Bot } from 'lucide-react';
import { Icon } from '@astryxdesign/core/Icon';
import { SideNavItem } from '@astryxdesign/core/SideNav';
import { HStack, VStack } from '@astryxdesign/core/Layout';
import { StatusDot } from '@astryxdesign/core/StatusDot';
import { Text } from '@astryxdesign/core/Text';
import { useStore } from '../state/store';
import { getAnimations } from '../state/configDefaults';
import { AGENT_GROUPS, agentGroup, type AgentGroup } from '../lib/agentGroups';

export const AGENT_DOT_VARIANT = {
  blocked: 'warning',
  working: 'accent',
  waiting: 'accent',
  done: 'success',
  idle: 'neutral',
} as const;
const RAIL_DOTS_PER_ROW = 4;

/** Agents nav entry with live per-state agent indicators. */
export function AgentNavItem({ collapsed, onClick }: { collapsed: boolean; onClick: () => void }) {
  const allSessions = useStore((state) => state.allSessions);
  const agentPanes = useStore((state) => state.agentPanes);
  const animate = useStore((state) => getAnimations(state.config)) || undefined;
  const agents = useMemo(() => {
    const list: { id: string; group: AgentGroup }[] = [];
    for (const session of allSessions)
      for (const win of session.windows)
        for (const pane of win.panes)
          if (agentPanes.has(pane.id)) list.push({ id: pane.id, group: agentGroup(pane.agent_status.state) });
    return list.sort((a, b) => AGENT_GROUPS.indexOf(a.group) - AGENT_GROUPS.indexOf(b.group));
  }, [allSessions, agentPanes]);
  const counts = AGENT_GROUPS.map(
    (group) => [group, agents.filter((agent) => agent.group === group).length] as const,
  ).filter(([, count]) => count > 0);
  const summary = counts.map(([group, count]) => `${count} ${group}`).join(', ');

  return (
    <SideNavItem
      // The collapsed rail's tooltip shows the label, so it carries the counts.
      label={collapsed && summary ? `Agents · ${summary}` : 'Agents'}
      aria-label={summary ? `Agents, ${summary}` : undefined}
      // Rows of rail dots grow the item rather than overflow it.
      className={collapsed ? 'h-auto min-h-(--size-element-md) py-1' : undefined}
      icon={
        <VStack hAlign="center" gap={0.5} className="shrink-0">
          <Icon icon={Bot} size="sm" color="secondary" />
          {collapsed && agents.length > 0 && (
            <div
              aria-hidden
              className="grid gap-0.5"
              style={{ gridTemplateColumns: `repeat(${Math.min(agents.length, RAIL_DOTS_PER_ROW)}, auto)` }}
            >
              {agents.map((agent) => (
                // Keyed by state so each transition replays the entrance.
                <span
                  key={`${agent.id}:${agent.group}`}
                  className="btm-agent-dot size-1 rounded-full"
                  data-agent-state={agent.group}
                  data-agent-animate={animate}
                />
              ))}
            </div>
          )}
        </VStack>
      }
      endContent={
        counts.length > 0 ? (
          <HStack gap={2} vAlign="center">
            {counts.map(([group, count]) => (
              <HStack key={group} gap={1} vAlign="center">
                <StatusDot
                  variant={AGENT_DOT_VARIANT[group]}
                  label={`${count} ${group}`}
                  tooltip={`${count} ${group}`}
                  className="btm-agent-dot"
                  data-agent-state={group}
                  data-agent-animate={animate}
                />
                <Text type="supporting" color="secondary" hasTabularNumbers>
                  {count}
                </Text>
              </HStack>
            ))}
          </HStack>
        ) : undefined
      }
      onClick={onClick}
    />
  );
}

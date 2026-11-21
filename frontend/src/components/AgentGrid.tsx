import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useLocation, useNavigate } from 'react-router-dom';
import { Dialog, DialogHeader } from '@astryxdesign/core/Dialog';
import { AlertDialog } from '@astryxdesign/core/AlertDialog';
import {
  Layout,
  LayoutContent,
  LayoutFooter,
  LayoutHeader,
  LayoutPanel,
  HStack,
  VStack,
} from '@astryxdesign/core/Layout';
import { List, ListItem } from '@astryxdesign/core/List';
import { Button } from '@astryxdesign/core/Button';
import { useMediaQuery } from '@astryxdesign/core/hooks';
import { TabList, Tab } from '@astryxdesign/core/TabList';
import { SelectableCard } from '@astryxdesign/core/SelectableCard';
import { Text } from '@astryxdesign/core/Text';
import { Kbd } from '@astryxdesign/core/Kbd';
import { EmptyState } from '@astryxdesign/core/EmptyState';
import { Icon } from '@astryxdesign/core/Icon';
import { IconButton } from '@astryxdesign/core/IconButton';
import { ToggleButton } from '@astryxdesign/core/ToggleButton';
import { StatusDot } from '@astryxdesign/core/StatusDot';
import { Bot, Eye, EyeOff, RotateCcw, X } from 'lucide-react';
import { githubKey, useStore } from '../state/store';
import { ClientMessage } from '../protocol/messages';
import type { AgentStatus, PaneGit } from '../state/types';
import { AgentStatusBadge } from './PaneTitleBar';
import { AGENTS } from './AgentIcon';
import { AGENT_DOT_VARIANT } from './AgentNavItem';
import { GithubBadge } from './GithubBadge';
import { AGENT_GROUP_LABELS, formatSince, groupAgents } from '../lib/agentGroups';
import { formatCost, formatTokens, recentUsage } from '../lib/agentUsage';
import { MirrorPane } from './MirrorPane';
import { KeyHint } from './KeyHint';
import { usePressHint } from '../hooks/usePressHint';
import { chromePalette } from '../lib/chrome-colors';
import { AgentUsageView } from './AgentUsageView';
import { cycleTabs, enterTabPanel } from '../lib/tabKeys';
import { NARROW_QUERY } from '../lib/touch';

interface Props {
  send: (msg: ClientMessage) => void;
}

const VIEWS = ['panes', 'usage'] as const;
type AgentView = (typeof VIEWS)[number];

/** Keys a focused tab handles itself; the rest still reach the grid. */
const TAB_KEYS = new Set(['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'Home', 'End', 'Enter', ' ']);

interface AgentPaneEntry {
  sessionId: string;
  sessionName: string;
  windowName: string;
  windowIndex: number;
  paneId: string;
  paneIndex: number;
  paneTitle: string | null;
  agentStatus: AgentStatus;
  agentId: number | null;
  /** Unix seconds. */
  since: number | null;
  git: PaneGit | null;
}

function buildEntries(
  allSessions: ReturnType<typeof useStore.getState>['allSessions'],
  activePaneIds: Set<string>,
): AgentPaneEntry[] {
  const entries: AgentPaneEntry[] = [];
  for (const session of allSessions) {
    session.windows.forEach((win, windowIndex) => {
      win.panes.forEach((pane, paneIndex) => {
        if (!activePaneIds.has(pane.id)) return;
        entries.push({
          sessionId: session.id,
          sessionName: session.name,
          windowName: win.name,
          windowIndex,
          paneId: pane.id,
          paneIndex,
          paneTitle: pane.title,
          agentStatus: pane.agent_status,
          agentId: pane.agent_id,
          since: pane.agent_since,
          git: pane.git,
        });
      });
    });
  }
  return entries;
}

const paneLabel = (entry: AgentPaneEntry) =>
  entry.paneTitle ||
  (entry.agentStatus.agent && (AGENTS.get(entry.agentStatus.agent)?.name ?? entry.agentStatus.agent)) ||
  `pane ${entry.paneIndex + 1}`;

/** What the agent says it is doing or waiting on. */
const paneDetail = (entry: AgentPaneEntry) => entry.agentStatus.message || entry.agentStatus.background;

// Agents whose live pane is shown, per browser. Pane ids are UUIDs, so stale ids never match.
const SHOWN_KEY = 'btmux-shown-agents';

function loadShown(): ReadonlySet<string> {
  try {
    const parsed = JSON.parse(localStorage.getItem(SHOWN_KEY) ?? '[]');
    return new Set(Array.isArray(parsed) ? parsed.filter((id) => typeof id === 'string') : []);
  } catch {
    return new Set();
  }
}

function saveShown(ids: ReadonlySet<string>) {
  try {
    localStorage.setItem(SHOWN_KEY, JSON.stringify([...ids]));
  } catch {
    // Storage may be unavailable; showing still works for this page.
  }
}

/** Unix seconds, ticking while `active`. */
function useNow(active: boolean) {
  const [now, setNow] = useState(() => Date.now() / 1000);
  useEffect(() => {
    if (!active) return;
    setNow(Date.now() / 1000);
    const timer = window.setInterval(() => setNow(Date.now() / 1000), 10_000);
    return () => clearInterval(timer);
  }, [active]);
  return now;
}

const SIDEBAR_WIDTH = 320;
// Per-tile card chrome around the terminal (padding, border, header and detail rows).
const CHROME_W = 24;
const CHROME_H = 84;
const FALLBACK_SIZE = { w: 640, h: 400 };
const MIN_SCALE = 0.1;
// How long a mirror may take to report its size before it gets the fallback.
const SIZE_WAIT_MS = 2000;

interface Size {
  w: number;
  h: number;
}

interface Packed {
  /** Entry indices per visual row. */
  lines: number[][];
  /** Terminal box size per entry index; `null` is not laid out yet. */
  sizes: (Size | null)[];
}

/**
 * Splits the sized tiles, in order, into the rows of least total height that fit
 * `width`; ties go to fewer, then fuller first, rows. Unlike greedy wrapping, that
 * height never shrinks as the scale grows, so `pack` can bisect it.
 */
function flow(naturals: (Size | null)[], scale: number, width: number, gap: number): Packed & { height: number } {
  const sizes = naturals.map((n) => n && { w: n.w * scale, h: n.h * scale });
  const order = sizes.flatMap((size, i) => (size ? [i] : []));
  const n = order.length;
  // best[k] lays out order[k..]: its height, row count, and where its second row starts.
  const best: { height: number; rows: number; next: number }[] = [];
  best[n] = { height: 0, rows: 0, next: n };
  for (let k = n - 1; k >= 0; k--) {
    const rows: { end: number; h: number }[] = [];
    let used = -gap;
    let rowH = 0;
    for (let j = k; j < n; j++) {
      const size = sizes[order[j]]!;
      used += gap + size.w + CHROME_W;
      // A tile wider than any row still gets one of its own.
      if (j > k && used > width) break;
      rowH = Math.max(rowH, size.h + CHROME_H);
      rows.push({ end: j + 1, h: rowH });
    }
    let pick: (typeof best)[number] | undefined;
    for (const { end, h } of rows.reverse()) {
      const height = h + (end < n ? gap + best[end].height : 0);
      const count = 1 + best[end].rows;
      if (!pick || height < pick.height - 0.01 || (height <= pick.height + 0.01 && count < pick.rows))
        pick = { height, rows: count, next: end };
    }
    best[k] = pick!;
  }
  const lines: number[][] = [];
  for (let k = 0; k < n; k = best[k].next) lines.push(order.slice(k, best[k].next));
  return { lines, sizes, height: best[0].height };
}

/** Largest shared scale (at most 1:1) at which all tiles fit without scrolling. */
function pack(naturals: (Size | null)[], width: number, height: number, gap: number): Packed {
  const widest = Math.max(1, ...naturals.map((n) => n?.w ?? 0));
  let lo = MIN_SCALE;
  let hi = Math.max(MIN_SCALE, Math.min(1, (width - CHROME_W) / widest));
  if (flow(naturals, hi, width, gap).height <= height) return flow(naturals, hi, width, gap);
  for (let i = 0; i < 20; i++) {
    const mid = (lo + hi) / 2;
    if (flow(naturals, mid, width, gap).height <= height) lo = mid;
    else hi = mid;
  }
  return flow(naturals, lo, width, gap);
}

/** Each tile's own wait for its mirror's size; a stuck mirror can't affect later tiles. */
function SizeDeadline({ paneId, onExpire }: { paneId: string; onExpire: (paneId: string) => void }) {
  useEffect(() => {
    const timer = window.setTimeout(() => onExpire(paneId), SIZE_WAIT_MS);
    return () => clearTimeout(timer);
  }, [paneId, onExpire]);
  return null;
}

/** Every agent, grouped by state, with live mirrors of the ones chosen. */
export function AgentGrid({ send }: Props) {
  const [view, setView] = useState<AgentView>('panes');
  const open = useStore((s) => s.agentGridOpen);
  const mounted = useStore((s) => s.agentGridMounted);
  const setOpen = useStore((s) => s.setAgentGridOpen);
  const allSessions = useStore((s) => s.allSessions);
  const agentPanes = useStore((s) => s.agentPanes);
  const telemetry = useStore((s) => s.agentTelemetry);
  const github = useStore((s) => s.github);
  const config = useStore((s) => s.config);
  const location = useLocation();
  const navigate = useNavigate();
  const isNarrow = useMediaQuery(NARROW_QUERY);
  const now = useNow(open);
  const entries = useMemo(() => buildEntries(allSessions, agentPanes), [allSessions, agentPanes]);
  const terminalBg = chromePalette(config?.theme ?? null).bodyBg;
  // The list, numbers, and selection go by urgency; tiles keep pane order, so they don't move.
  const groups = useMemo(
    () =>
      groupAgents(
        entries,
        (entry) => entry.agentStatus.state,
        (entry) => entry.since,
      ),
    [entries],
  );
  const ordered = useMemo(() => groups.flatMap(([, members]) => members), [groups]);

  // Live panes are opt-in; hidden agents stay in the sidebar without a mirror.
  const [shownIds, setShownState] = useState<ReadonlySet<string>>(loadShown);
  const setShown = (ids: Iterable<string>) => {
    const panes = new Set(allSessions.flatMap((s) => s.windows.flatMap((w) => w.panes.map((p) => p.id))));
    const next = new Set([...ids].filter((id) => panes.has(id)));
    setShownState(next);
    saveShown(next);
  };
  const shown = useMemo(() => entries.filter((entry) => shownIds.has(entry.paneId)), [entries, shownIds]);

  const [mirrorsReady, setMirrorsReady] = useState(false);
  const hasEverBeenReady = useRef(false);
  useEffect(() => {
    if (open && !hasEverBeenReady.current) {
      requestAnimationFrame(() => {
        requestAnimationFrame(() => {
          setMirrorsReady(true);
          hasEverBeenReady.current = true;
        });
      });
    }
  }, [open]);

  // Tiles are each terminal's natural size times one shared scale, so fonts
  // match across tiles (a mirror can't change the PTY size).
  const [naturals, setNaturals] = useState<Record<string, Size>>({});
  const reportSize = useCallback((paneId: string, w: number, h: number) => {
    setNaturals((prev) => {
      const old = prev[paneId];
      return old && old.w === w && old.h === h ? prev : { ...prev, [paneId]: { w, h } };
    });
  }, []);
  // Tiles first appear together, once sized. An agent that starts later (or is
  // restored) joins once sized, so each change of the tile set is one layout change.
  const [timedOutPanes, setTimedOutPanes] = useState<ReadonlySet<string>>(new Set());
  const sizeTimedOut = useCallback((paneId: string) => setTimedOutPanes((prev) => new Set(prev).add(paneId)), []);
  const allSized = shown.every((entry) => naturals[entry.paneId] || timedOutPanes.has(entry.paneId));
  // Tiles shown together (Show all) join together, once all are sized.
  const [laidOut, setLaidOut] = useState<ReadonlySet<string>>(new Set());
  const placed = useMemo(
    () => new Set(shown.map((entry) => entry.paneId).filter((id) => allSized || laidOut.has(id))),
    [shown, allSized, laidOut],
  );
  useEffect(() => {
    if (allSized)
      setLaidOut((prev) => (prev.size === placed.size && [...placed].every((id) => prev.has(id)) ? prev : placed));
  }, [allSized, placed]);
  const [tilesShown, setTilesShown] = useState(false);
  useEffect(() => {
    if (open) {
      if (allSized) setTilesShown(true);
    } else {
      setTilesShown(false);
      setTimedOutPanes(new Set());
    }
  }, [open, allSized]);
  const [area, setArea] = useState<{ w: number; h: number; gap: number } | null>(null);
  const observerRef = useRef<ResizeObserver | null>(null);
  const areaRef = useCallback((el: HTMLElement | null) => {
    observerRef.current?.disconnect();
    observerRef.current = null;
    if (!el) return;
    const measure = () => {
      // The stack's token gap, resolved to px.
      const gap = parseFloat(getComputedStyle(el).columnGap) || 0;
      if (el.clientWidth > 0 && el.clientHeight > 0) setArea({ w: el.clientWidth, h: el.clientHeight, gap });
    };
    measure();
    observerRef.current = new ResizeObserver(measure);
    observerRef.current.observe(el);
  }, []);
  // Indices in `packed` are into `shown`.
  const packed = useMemo(
    () =>
      area && area.w > 0 && area.h > 0
        ? pack(
            shown.map((entry) =>
              placed.has(entry.paneId)
                ? (naturals[entry.paneId] ?? (timedOutPanes.has(entry.paneId) ? FALLBACK_SIZE : null))
                : null,
            ),
            area.w,
            area.h,
            area.gap,
          )
        : null,
    [area, shown, placed, naturals, timedOutPanes],
  );
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [killing, setKilling] = useState<AgentPaneEntry | null>(null);
  const dialogRef = useRef<HTMLDialogElement>(null);
  // A synthetic Tab moves no focus, so hints must reach the dialog's handler.
  const focusDialog = useCallback(() => dialogRef.current?.focus(), []);
  const pressHint = usePressHint(dialogRef, focusDialog);
  const wasOpen = useRef(false);

  useEffect(() => {
    if (open && !wasOpen.current) {
      const match = location.pathname.match(/^\/s\/([^/]+)/);
      const sessionName = match ? decodeURIComponent(match[1]) : null;
      const session = sessionName ? allSessions.find((s) => s.name === sessionName) : null;
      const activeWindow = session?.windows[session.active_window];
      const activePaneId = activeWindow?.panes[activeWindow.active_pane]?.id;
      const active = entries.find((entry) => entry.paneId === activePaneId);
      setSelectedId((active ?? ordered[0])?.paneId ?? null);
    }
    wasOpen.current = open;
  }, [open, entries, ordered, location.pathname, allSessions]);

  // Selection is by pane, over all agents; a gone agent falls back to the most urgent.
  const selectedIdx = Math.max(
    0,
    ordered.findIndex((entry) => entry.paneId === selectedId),
  );
  const selected = ordered[selectedIdx];

  // Keeps the selection in view, and focus on its row when a row has focus, so Enter opens it.
  // By id: updates rebuild entries, and must not scroll the list back.
  const selectedPane = selected?.paneId;
  useEffect(() => {
    if (!open || !selectedPane) return;
    const row = dialogRef.current?.querySelector<HTMLElement>(`[data-agent-row="${selectedPane}"]`);
    if (!row) return;
    row.scrollIntoView({ block: 'nearest' });
    const focused = document.activeElement?.closest('[data-agent-row]');
    if (focused && focused !== row) row.querySelector<HTMLElement>('button:not([aria-pressed])')?.focus();
  }, [open, selectedPane]);

  // SessionPane refocuses the active pane on close.
  const cancel = () => setOpen(false);

  const select = (entry: AgentPaneEntry | undefined) => {
    if (!entry) return;
    send({ type: 'switch_window', session_id: entry.sessionId, index: entry.windowIndex });
    send({ type: 'select_pane', session_id: entry.sessionId, pane_id: entry.paneId });
    const session = allSessions.find((item) => item.id === entry.sessionId);
    navigate(`/s/${encodeURIComponent(session?.name ?? entry.sessionName)}/w/${encodeURIComponent(entry.windowName)}`);
    setOpen(false);
  };

  const reset = (entry: AgentPaneEntry | undefined) => {
    if (entry) send({ type: 'reset_agent', pane_id: entry.paneId });
  };

  const confirmKill = (entry: AgentPaneEntry | undefined) => {
    if (entry?.agentId != null) setKilling(entry);
  };

  // The agent the confirmation names, while it still runs.
  const killTarget =
    open && killing ? entries.find((e) => e.paneId === killing.paneId && e.agentId === killing.agentId) : undefined;
  useEffect(() => {
    if (killing && !killTarget) setKilling(null);
  }, [killing, killTarget]);

  const closeKill = (confirmed: boolean) => {
    // The server sends both Ctrl-Cs, only to this agent instance.
    if (confirmed && killTarget?.agentId != null)
      send({ type: 'kill_agent', pane_id: killTarget.paneId, agent_id: killTarget.agentId });
    setKilling(null);
    // After the alert returns focus to its trigger, so the grid keeps its keys.
    requestAnimationFrame(() => dialogRef.current?.focus());
  };

  const toggleShown = (entry: AgentPaneEntry) => {
    const next = new Set(shownIds);
    if (!next.delete(entry.paneId)) next.add(entry.paneId);
    setShown(next);
  };

  // Shows only this agent; again, shows them all.
  const toggleOnly = (entry: AgentPaneEntry) => {
    const alone = shown.length === 1 && shown[0].paneId === entry.paneId;
    setShown(alone ? entries.map((other) => other.paneId) : [entry.paneId]);
  };

  const changeView = (value: AgentView) => {
    setView(value);
    // A tab keeping focus would take the grid's keys.
    dialogRef.current?.focus();
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLElement>) => {
    e.stopPropagation();
    const n = entries.length;
    if (e.key === 'Escape') {
      e.preventDefault();
      cancel();
      return;
    }
    if (cycleTabs(e, VIEWS, view, changeView)) return;
    if (view !== 'panes') {
      enterTabPanel(e);
      return;
    }
    if (TAB_KEYS.has(e.key) && (e.target as Element).closest('[role="tablist"]')) return;
    if (n === 0 || !selected) return;
    if (e.key === 'Enter' || e.key === ' ') {
      const target = e.target as Element;
      // Space on a focused row shows or hides it; the row's button would go to the pane.
      const row = e.key === ' ' && !target.closest('[aria-pressed]') && target.closest('[data-agent-row]');
      const rowEntry = row && ordered.find((entry) => entry.paneId === row.getAttribute('data-agent-row'));
      if (rowEntry) {
        e.preventDefault();
        toggleShown(rowEntry);
        return;
      }
      // A focused control activates itself.
      if (target.closest('button, a[href], input')) return;
      e.preventDefault();
      if (e.key === ' ') toggleShown(selected);
      else select(selected);
      return;
    }
    const right = e.key === 'ArrowRight' || e.key === 'l';
    const left = e.key === 'ArrowLeft' || e.key === 'h';
    const down = e.key === 'ArrowDown' || e.key === 'j';
    const up = e.key === 'ArrowUp' || e.key === 'k';
    if (down || up) {
      e.preventDefault();
      const next = ordered[selectedIdx + (down ? 1 : -1)];
      if (next) setSelectedId(next.paneId);
      return;
    }
    if (left || right) {
      e.preventDefault();
      // Left and right move between tiles; from a hidden agent, to the first or last.
      const pos = shown.indexOf(selected);
      const next = pos < 0 ? shown[right ? 0 : shown.length - 1] : shown[pos + (right ? 1 : -1)];
      if (next) setSelectedId(next.paneId);
      return;
    }
    if (!e.metaKey && !e.ctrlKey && !e.altKey) {
      const action = { r: reset, o: toggleOnly, x: confirmKill }[e.key];
      if (action) {
        e.preventDefault();
        action(selected);
        return;
      }
    }
    if (e.key >= '1' && e.key <= '9') {
      const idx = Number(e.key) - 1;
      if (idx < n) {
        e.preventDefault();
        setSelectedId(ordered[idx].paneId);
      }
    }
  };

  if (!mounted) return null;

  const allShown = shown.length === entries.length;
  const number = (entry: AgentPaneEntry) => ordered.indexOf(entry);
  const usage = (entry: AgentPaneEntry) => {
    const pane = telemetry[entry.paneId];
    if (!pane) return null;
    const totals = recentUsage(pane, now);
    const tokens = totals.input_tokens + totals.output_tokens;
    const parts = [
      totals.cost_usd != null && formatCost(totals.cost_usd, 2),
      tokens > 0 && `${formatTokens(tokens)} tokens`,
    ];
    return parts.filter(Boolean).join(' · ') || null;
  };
  const githubStatus = (entry: AgentPaneEntry) =>
    entry.git?.branch ? github.get(githubKey(entry.git.repo_id, entry.git.branch)) : undefined;
  const since = (entry: AgentPaneEntry) => (entry.since == null ? null : formatSince(entry.since, now));

  const sidebar = (
    <VStack gap={3}>
      <HStack gap={2} vAlign="center" hAlign="between">
        <Text type="supporting" hasTabularNumbers>
          {shown.length ? `${shown.length} of ${entries.length} shown` : `${entries.length} agents`}
        </Text>
        <Button
          label={allShown ? 'Hide all' : 'Show all'}
          variant="ghost"
          size="sm"
          onClick={() => {
            setShown(allShown ? [] : entries.map((entry) => entry.paneId));
            dialogRef.current?.focus();
          }}
        />
      </HStack>
      {groups.map(([group, members]) => (
        <List
          key={group}
          density="compact"
          data-agent-group={group}
          header={
            <HStack gap={2} vAlign="center">
              <StatusDot
                variant={AGENT_DOT_VARIANT[group]}
                label={AGENT_GROUP_LABELS[group]}
                className="btm-agent-dot"
                data-agent-state={group}
              />
              <Text type="label">{AGENT_GROUP_LABELS[group]}</Text>
              <Text type="supporting" color="secondary" hasTabularNumbers>
                {members.length}
              </Text>
            </HStack>
          }
        >
          {members.map((entry) => {
            const isShown = shownIds.has(entry.paneId);
            const i = number(entry);
            const agent = entry.agentStatus.agent ? AGENTS.get(entry.agentStatus.agent) : undefined;
            const detail = paneDetail(entry);
            const branch = entry.git?.branch;
            const time = since(entry);
            return (
              <ListItem
                key={entry.paneId}
                data-agent-row={entry.paneId}
                data-shown={isShown || undefined}
                isSelected={entry === selected}
                align="start"
                startContent={
                  <Icon
                    icon={agent?.logo ?? Bot}
                    size="sm"
                    color="primary"
                    label={agent?.name ?? entry.agentStatus.agent ?? 'Agent'}
                  />
                }
                label={<Text maxLines={1}>{paneLabel(entry)}</Text>}
                description={
                  <VStack gap={0.5}>
                    {detail && (
                      <Text type="supporting" maxLines={2}>
                        {detail}
                      </Text>
                    )}
                    <Text type="supporting" color="secondary" maxLines={1} className="btm-mono">
                      {entry.sessionName} › {entry.windowName}
                      {branch && ` · ${branch}`}
                    </Text>
                  </VStack>
                }
                endContent={
                  <HStack gap={1} vAlign="center">
                    {time && (
                      <Text
                        type="supporting"
                        color="secondary"
                        hasTabularNumbers
                        aria-label={`${AGENT_GROUP_LABELS[group]} for ${time}`}
                      >
                        {time}
                      </Text>
                    )}
                    {i < 9 && <Kbd keys={String(i + 1)} />}
                    <ToggleButton
                      label={isShown ? 'Hide live pane' : 'Show live pane'}
                      tooltip={isShown ? 'Hide live pane (space)' : 'Show live pane (space)'}
                      icon={<EyeOff />}
                      pressedIcon={<Eye />}
                      isIconOnly
                      size="sm"
                      isPressed={isShown}
                      data-agent-eye
                      onPressedChange={(_, e) => {
                        e.stopPropagation();
                        setSelectedId(entry.paneId);
                        toggleShown(entry);
                        // A focused button would take Enter from the grid.
                        dialogRef.current?.focus();
                      }}
                    />
                  </HStack>
                }
                onClick={() => select(entry)}
              />
            );
          })}
        </List>
      ))}
    </VStack>
  );

  const tiles = (
    // Rows are `packed.lines`, ended by breaks rather than wrapped by width; safe
    // alignment keeps an overflowing first row or column reachable by scrolling.
    <HStack
      ref={areaRef}
      data-agent-tiles
      wrap="wrap"
      vAlign="center"
      isScrollable
      className={`h-full min-h-0 min-w-0 flex-1 content-center-safe justify-center-safe gap-x-3 ${tilesShown || allSized ? '' : 'opacity-0'}`}
    >
      {shown.length === 0 && (
        <EmptyState
          icon={<Icon icon={Eye} size="lg" color="secondary" />}
          title="No live panes shown"
          description="Use an agent's eye button, or select it and press Space, to watch it here."
        />
      )}
      {packed &&
        shown.flatMap((entry, i) => {
          const n = number(entry);
          const size = packed.sizes[i];
          const detail = paneDetail(entry);
          const status = githubStatus(entry);
          const spent = usage(entry);
          const time = since(entry);
          // The row gap, as a line of its own (gap-x-3's height).
          const rowBreak = packed.lines.some((line, row) => row > 0 && line[0] === i) && (
            <div key={`break-${entry.paneId}`} aria-hidden className="h-3 basis-full" />
          );
          return [
            rowBreak,
            <div
              key={entry.paneId}
              data-agent-tile={entry.paneId}
              // An unsized tile keeps its mirror mounted, which reports the size.
              style={
                size
                  ? { width: size.w + CHROME_W, height: size.h + CHROME_H }
                  : {
                      position: 'absolute',
                      width: FALLBACK_SIZE.w,
                      height: FALLBACK_SIZE.h,
                      opacity: 0,
                      pointerEvents: 'none',
                    }
              }
            >
              <SelectableCard
                label={`${entry.sessionName} › ${entry.windowName} › ${paneLabel(entry)}`}
                isSelected={entry === selected}
                onChange={() => select(entry)}
                padding={2}
                height="100%"
              >
                <VStack gap={2} className="h-full min-h-0">
                  <HStack gap={2} vAlign="center" className="min-w-0">
                    {n < 9 && <Kbd keys={String(n + 1)} />}
                    <Text maxLines={1} className="min-w-0 flex-1">
                      <Text color="secondary" className="btm-mono">
                        {entry.sessionName}
                      </Text>{' '}
                      <Text className="btm-mono">{entry.windowName}</Text>{' '}
                      <Text color="secondary">{paneLabel(entry)}</Text>
                    </Text>
                    <HStack gap={1} vAlign="center" className="shrink-0">
                      <AgentStatusBadge status={entry.agentStatus} />
                      {time && (
                        <Text type="supporting" color="secondary" hasTabularNumbers>
                          {time}
                        </Text>
                      )}
                      <IconButton
                        label="Reset agent state"
                        tooltip="Reset agent state (r)"
                        icon={<RotateCcw />}
                        variant="ghost"
                        size="sm"
                        onClick={(e) => {
                          e.preventDefault();
                          e.stopPropagation();
                          reset(entry);
                          // A focused button would keep Tab inside the panel.
                          dialogRef.current?.focus();
                        }}
                      />
                      <IconButton
                        label="Hide live pane"
                        tooltip="Hide live pane (space)"
                        icon={<EyeOff />}
                        variant="ghost"
                        size="sm"
                        onClick={(e) => {
                          e.preventDefault();
                          e.stopPropagation();
                          toggleShown(entry);
                          dialogRef.current?.focus();
                        }}
                      />
                      <IconButton
                        label="Kill agent session"
                        tooltip="Kill agent session (x)"
                        icon={<X />}
                        variant="ghost"
                        size="sm"
                        onClick={(e) => {
                          e.preventDefault();
                          e.stopPropagation();
                          confirmKill(entry);
                        }}
                      />
                    </HStack>
                  </HStack>
                  <HStack gap={2} vAlign="center" className="h-6 min-w-0 shrink-0" data-agent-detail>
                    <Text type="supporting" color="secondary" maxLines={1} className="min-w-0 flex-1">
                      {detail}
                    </Text>
                    {status ? (
                      <GithubBadge status={status} size="sm" />
                    ) : (
                      entry.git?.branch && (
                        <Text type="supporting" color="secondary" maxLines={1} className="btm-mono max-w-1/3 min-w-0">
                          {entry.git.branch}
                        </Text>
                      )
                    )}
                    {spent && (
                      <Text
                        type="supporting"
                        color="secondary"
                        hasTabularNumbers
                        className="shrink-0"
                        aria-label={`Reported usage in the last 24 hours: ${spent}`}
                      >
                        {spent}
                      </Text>
                    )}
                  </HStack>
                  <VStack
                    data-agent-mirror
                    // Concentric with the card's corners, 8px (padding 2) inside them.
                    className="relative min-h-0 flex-1 overflow-hidden rounded-[max(0px,calc(var(--radius-container)-var(--spacing-2)))]"
                    style={{ background: terminalBg }}
                  >
                    {/* Only while shown: a hidden mirror can't report its size, and closing clears expiries. */}
                    {open && view === 'panes' && !naturals[entry.paneId] && (
                      <SizeDeadline paneId={entry.paneId} onExpire={sizeTimedOut} />
                    )}
                    {mirrorsReady && (
                      <MirrorPane
                        paneId={entry.paneId}
                        visible={open && view === 'panes'}
                        onNaturalSize={(w, h) => reportSize(entry.paneId, w, h)}
                      />
                    )}
                  </VStack>
                </VStack>
              </SelectableCard>
            </div>,
          ];
        })}
    </HStack>
  );

  const hasSidebar = view === 'panes' && entries.length > 0;

  return (
    <Dialog
      ref={dialogRef}
      tabIndex={-1}
      isOpen={open}
      onOpenChange={(value) => {
        if (!value) cancel();
      }}
      variant="fullscreen"
      onKeyDown={onKeyDown}
      onFocus={(e) => {
        // MirrorPane's Terminal.open() focuses its textarea; keep keys on the dialog.
        if ((e.target as Element).closest('[data-agent-mirror]')) dialogRef.current?.focus();
      }}
    >
      <AlertDialog
        isOpen={killTarget !== undefined}
        onOpenChange={(value) => {
          if (!value) closeKill(false);
        }}
        title="Kill agent session?"
        description={
          killTarget
            ? `Sends Ctrl-C twice to ${killTarget.sessionName} › ${killTarget.windowName} › ${paneLabel(killTarget)}, which exits Claude Code and Codex.`
            : ''
        }
        actionLabel="Kill session"
        onAction={() => closeKill(true)}
        onKeyDown={(e) => {
          // React bubbles portal events to the grid's handler.
          e.stopPropagation();
          if (e.key === 'y' || e.key === 'n') {
            e.preventDefault();
            closeKill(e.key === 'y');
          }
        }}
      />
      <Layout
        header={
          <>
            <DialogHeader
              title="Agents"
              onOpenChange={(value) => {
                if (!value) cancel();
              }}
            />
            <LayoutHeader paddingBlockEnd={0}>
              <TabList
                value={view}
                onChange={(value) => changeView(value as AgentView)}
                role="tablist"
                hasDivider
                isFullBleed
              >
                <Tab value="panes" label="Live panes" panelId="agents-panes" />
                <Tab value="usage" label="Usage" panelId="agents-usage" />
              </TabList>
            </LayoutHeader>
          </>
        }
        start={
          hasSidebar && !isNarrow ? (
            <LayoutPanel width={SIDEBAR_WIDTH} label="Agent list">
              {sidebar}
            </LayoutPanel>
          ) : undefined
        }
        content={
          <LayoutContent padding={4}>
            <VStack
              id="agents-panes"
              role="tabpanel"
              aria-label="Live panes"
              gap={2}
              className={view === 'panes' ? 'h-full min-h-0' : 'hidden'}
            >
              {entries.length === 0 ? (
                <VStack hAlign="center" vAlign="center" className="h-full">
                  <EmptyState
                    icon={<Icon icon={Bot} size="lg" color="secondary" />}
                    title="No agents detected"
                    description="Start Claude Code, Codex, Gemini, Antigravity, or OpenCode in a pane."
                  />
                </VStack>
              ) : (
                <>
                  {/* Narrow screens stack the list above the tiles; without tiles, it takes the height. */}
                  {isNarrow && (
                    <VStack
                      className={shown.length ? 'max-h-1/3 shrink-0 overflow-auto' : 'min-h-0 flex-1 overflow-auto'}
                    >
                      {sidebar}
                    </VStack>
                  )}
                  {(shown.length > 0 || !isNarrow) && tiles}
                </>
              )}
            </VStack>
            {view === 'usage' && open && (
              <VStack id="agents-usage" role="tabpanel" aria-label="Usage" className="h-full">
                <AgentUsageView />
              </VStack>
            )}
          </LayoutContent>
        }
        footer={
          <LayoutFooter hasDivider>
            <HStack gap={4} wrap="wrap">
              {(view === 'panes' && entries.length ? HINTS : CLOSE_HINTS).map(([keys, label]) => (
                <KeyHint key={label} keys={keys} label={label} onPress={pressHint} />
              ))}
            </HStack>
          </LayoutFooter>
        }
      />
    </Dialog>
  );
}

const HINTS: [string[], string][] = [
  [['tab'], 'next tab'],
  [['up', 'down'], 'list'],
  [['left', 'right'], 'tiles'],
  [['1', '9'], 'select agent'],
  [['enter'], 'go to pane'],
  [['space'], 'show/hide'],
  [['o'], 'only'],
  [['r'], 'reset agent state'],
  [['x'], 'kill agent session'],
  [['esc'], 'close'],
];

const CLOSE_HINTS: [string[], string][] = [
  [['tab'], 'next tab'],
  [['esc'], 'close'],
];

import type { LayoutRect } from '../state/types';
import type { PaneSnapshot, Theme } from '../generated/protocol';

const WIDTH = 108;
const HEIGHT = 81;

/** A pane's dot: an unread notification, else an agent needing a look or at work. */
function dotColor(pane: PaneSnapshot, theme: Theme): string | null {
  const level =
    pane.unread?.level ??
    (pane.agent_status.state === 'blocked'
      ? 'attention'
      : pane.agent_status.state === 'done'
        ? 'success'
        : pane.agent_status.state === 'working'
          ? 'info'
          : null);
  if (level === 'error') return theme.red;
  if (level === 'attention') return theme.yellow;
  if (level === 'success') return theme.green;
  if (level === 'info') return theme.blue;
  return null;
}

/** Phones show one pane at a time; this map of the window's layout selects another with a tap. */
export function PaneMinimap({
  rects,
  panes,
  activePaneId,
  numbers,
  theme,
  onSelect,
  docked = false,
}: {
  rects: LayoutRect[];
  panes: PaneSnapshot[];
  activePaneId: string | null;
  numbers: Map<string, number>;
  theme: Theme;
  onSelect: (paneId: string) => void;
  /** In PhoneDock's flow, rather than floating over the pane's corner. */
  docked?: boolean;
}) {
  const byId = new Map(panes.map((pane) => [pane.id, pane]));
  return (
    <nav
      aria-label="Panes"
      data-pane-minimap
      className="btm-toolbar-glass btm-mono"
      // Keep the keyboard open: a tap here mustn't take focus from the terminal.
      onMouseDown={(event) => event.preventDefault()}
      style={{
        ...(docked
          ? // As tall as the buttons beside it.
            { position: 'relative', height: '100%' }
          : {
              position: 'absolute',
              right: 10,
              bottom: 10,
              // Above the pane-covering surfaces, like the pane numbers.
              zIndex: 36,
            }),
        width: WIDTH,
        ...(docked ? {} : { height: HEIGHT }),
        padding: 4,
        borderRadius: 'var(--radius-element)',
        border: `1px solid ${theme.selectionBackground}`,
        boxShadow: '0 2px 8px rgb(0 0 0 / 0.35)',
      }}
    >
      <div style={{ position: 'relative', width: '100%', height: '100%' }}>
        {rects.map((rect) => {
          const pane = byId.get(rect.paneId);
          if (!pane) return null;
          const active = rect.paneId === activePaneId;
          const dot = dotColor(pane, theme);
          const n = numbers.get(rect.paneId) ?? 0;
          return (
            <button
              key={rect.paneId}
              type="button"
              aria-label={`Pane ${n}${pane.title ? `: ${pane.title}` : ''}`}
              aria-current={active || undefined}
              onClick={() => onSelect(rect.paneId)}
              style={{
                position: 'absolute',
                top: `${rect.top}%`,
                left: `${rect.left}%`,
                width: `${rect.width}%`,
                height: `${rect.height}%`,
                padding: 1,
                background: 'none',
                border: 'none',
                cursor: 'pointer',
              }}
            >
              <span
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  justifyContent: 'center',
                  position: 'relative',
                  width: '100%',
                  height: '100%',
                  borderRadius: 3,
                  background: active ? theme.cursor : `color-mix(in srgb, ${theme.foreground} 18%, transparent)`,
                  color: active ? theme.background : theme.foreground,
                  fontSize: 13,
                  lineHeight: 1,
                  fontWeight: active ? 'bold' : undefined,
                }}
              >
                {n}
                {dot && (
                  <span
                    style={{
                      position: 'absolute',
                      top: 3,
                      right: 3,
                      width: 7,
                      height: 7,
                      borderRadius: '50%',
                      background: dot,
                    }}
                  />
                )}
              </span>
            </button>
          );
        })}
      </div>
    </nav>
  );
}

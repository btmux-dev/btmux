import type { LucideIcon } from 'lucide-react';
import { Cpu, HardDrive, MemoryStick, Network } from 'lucide-react';
import { useSysStats } from '../hooks/useSysStats';
import type { ChromePalette } from '../lib/chrome-colors';
import { mix, withAlpha } from '../lib/chrome-colors';
import { readableColor } from '../lib/astryx-theme';
import { CONNECTION_STATE_LABEL } from '../lib/connectionState';

interface Props {
  c: ChromePalette;
  barH: number;
  font: number;
  animations: boolean;
}

// base03 (fgMuted) is a comment color and too faint for live values.
const VALUE_COLOR = 'var(--color-text-secondary)';

const NETWORK_SATURATION_FLOOR_BPS = 1_000;
const NETWORK_FULL_SATURATION_BPS = 10_000_000;

/**
 * Map bytes/s onto a perceptual 0–1 saturation scale. Network rates span many
 * orders of magnitude, so a logarithmic curve keeps light interactive traffic
 * visible while reserving full color for sustained transfers.
 */
function networkSaturation(bytesPerSecond: number): number {
  if (!Number.isFinite(bytesPerSecond) || bytesPerSecond <= 0) return 0;
  return Math.min(
    1,
    Math.log1p(bytesPerSecond / NETWORK_SATURATION_FLOOR_BPS) /
      Math.log1p(NETWORK_FULL_SATURATION_BPS / NETWORK_SATURATION_FLOOR_BPS),
  );
}

function fmtBytes(bytes: number): string {
  if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(1)}G`;
  if (bytes >= 1e6) return `${(bytes / 1e6).toFixed(0)}M`;
  if (bytes >= 1e3) return `${(bytes / 1e3).toFixed(0)}K`;
  return `${bytes}B`;
}

function fmtPct(used: number, total: number): string {
  if (!total) return '0%';
  return `${Math.round((used / total) * 100)}%`;
}

// Non-text contrast for chart marks (WCAG 1.4.11), so critical bars stay
// visible whatever the palette's red.
const MARK_CONTRAST = 3;

function CpuBars({ cpu, c, barH }: { cpu: number[]; c: ChromePalette; barH: number }) {
  const innerH = Math.round(barH * 0.55);
  const barW = Math.max(2, Math.round(barH * 0.13));
  const gap = Math.max(1, Math.round(barW * 0.4));
  const danger = readableColor(c.danger, c.bodyBg, MARK_CONTRAST);

  return (
    <div
      style={{
        display: 'flex',
        alignItems: 'flex-end',
        gap: `${gap}px`,
        height: `${innerH}px`,
      }}
    >
      {cpu.map((pct, i) => {
        // Color: green → yellow → red based on load
        const t = Math.min(1, pct / 100);
        const color = t < 0.6 ? mix(c.accent, c.warn, t / 0.6) : mix(c.warn, danger, (t - 0.6) / 0.4);
        const h = Math.max(1, Math.round(innerH * Math.max(0.04, t)));
        return (
          <div
            key={i}
            title={`CPU ${i}: ${pct.toFixed(0)}%`}
            style={{
              width: `${barW}px`,
              height: `${h}px`,
              background: color,
              borderRadius: '1px',
              flexShrink: 0,
            }}
          />
        );
      })}
    </div>
  );
}

interface UsageMeterProps {
  icon: LucideIcon;
  label: string;
  used: number;
  total: number;
  /** Spoken and shown on hover, e.g. "3.1G of 16.0G used". */
  detail: string;
  c: ChromePalette;
  barH: number;
  iconSize: number;
}

function UsageMeter({ icon: MeterIcon, label, used, total, detail, c, barH, iconSize }: UsageMeterProps) {
  const pct = total ? Math.min(1, used / total) : 0;
  const color =
    pct > 0.85 ? readableColor(c.danger, mix(c.bodyBg, c.fgDim, 0.2), MARK_CONTRAST) : pct > 0.65 ? c.warn : c.accent;
  const barW = Math.round(barH * 0.9);
  const innerH = Math.round(barH * 0.55);
  const percent = fmtPct(used, total);

  return (
    <div
      role="meter"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(pct * 100)}
      aria-valuetext={`${percent}, ${detail}`}
      title={`${label}: ${detail}`}
      style={{ display: 'flex', alignItems: 'center', gap: '5px' }}
    >
      <span aria-hidden style={{ display: 'flex', color: c.fgMuted, flexShrink: 0 }}>
        <MeterIcon size={iconSize} />
      </span>
      <div
        aria-hidden
        style={{
          position: 'relative',
          width: `${barW}px`,
          height: `${Math.round(innerH * 0.55)}px`,
          background: withAlpha(c.fgDim, 0.2),
          borderRadius: '2px',
          overflow: 'hidden',
        }}
      >
        <div
          style={{
            position: 'absolute',
            inset: 0,
            width: `${Math.round(barW * pct)}px`,
            background: color,
            borderRadius: '2px',
          }}
        />
      </div>
      <span aria-hidden style={{ color: VALUE_COLOR, minWidth: '4ch', fontVariantNumeric: 'tabular-nums' }}>
        {percent}
      </span>
    </div>
  );
}

export function SysStatBar({ c, barH, font, animations }: Props) {
  const { stats, state } = useSysStats();

  if (!stats) {
    return (
      <div
        role="status"
        className={state !== 'connected' && animations ? 'animate-pulse' : undefined}
        style={{
          display: 'flex',
          alignItems: 'center',
          height: '100%',
          padding: '0 13px',
          color: c.fgDim,
          borderLeft: `1px solid ${c.borderDim}`,
          fontSize: `${font}px`,
        }}
      >
        {state === 'connected' ? '—' : CONNECTION_STATE_LABEL[state]}
      </div>
    );
  }

  const iconSize = Math.max(11, font + 1);
  const netRxSaturation = networkSaturation(stats.net_rx);
  const netTxSaturation = networkSaturation(stats.net_tx);

  return (
    <div
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: '16px',
        height: '100%',
        padding: '0 13px',
        borderLeft: `1px solid ${c.borderDim}`,
        fontSize: `${font}px`,
      }}
    >
      {/* CPU bar chart */}
      <div style={{ display: 'flex', alignItems: 'center', gap: '5px' }}>
        <span
          title={`CPU: ${Math.round(stats.cpu.reduce((a, b) => a + b, 0) / Math.max(1, stats.cpu.length))}% average across ${stats.cpu.length} cores`}
          style={{ display: 'flex', color: c.fgMuted, flexShrink: 0 }}
        >
          <Cpu size={iconSize} aria-label="CPU" />
        </span>
        <CpuBars cpu={stats.cpu} c={c} barH={barH} />
      </div>

      <UsageMeter
        icon={MemoryStick}
        label="Memory"
        used={stats.mem_used}
        total={stats.mem_total}
        detail={`${fmtBytes(stats.mem_used)} of ${fmtBytes(stats.mem_total)} used`}
        c={c}
        barH={barH}
        iconSize={iconSize}
      />
      {stats.disk && (
        <UsageMeter
          icon={HardDrive}
          label="Server home volume"
          used={stats.disk.total - stats.disk.available}
          total={stats.disk.total}
          detail={`${fmtBytes(stats.disk.available)} free of ${fmtBytes(stats.disk.total)}`}
          c={c}
          barH={barH}
          iconSize={iconSize}
        />
      )}

      {/* Network */}
      <div style={{ display: 'flex', alignItems: 'center', gap: '5px', color: c.fgDim }}>
        <span
          title={`Network: ↓ ${fmtBytes(stats.net_rx)}/s, ↑ ${fmtBytes(stats.net_tx)}/s`}
          style={{ display: 'flex', color: c.fgMuted, flexShrink: 0 }}
        >
          <Network size={iconSize} aria-label="Network" />
        </span>
        <div style={{ display: 'flex', alignItems: 'center', gap: '2px' }}>
          <span
            title={`Receive: ${fmtBytes(stats.net_rx)}/s`}
            style={{
              color: c.accent,
              filter: `saturate(${netRxSaturation})`,
              fontSize: `${Math.max(8, font - 1)}px`,
              transition: 'filter 200ms ease-out',
            }}
          >
            ↓
          </span>
          <span style={{ color: VALUE_COLOR, minWidth: '4ch', fontVariantNumeric: 'tabular-nums' }}>
            {fmtBytes(stats.net_rx)}
          </span>
        </div>
        <div style={{ display: 'flex', alignItems: 'center', gap: '2px' }}>
          <span
            title={`Transmit: ${fmtBytes(stats.net_tx)}/s`}
            style={{
              color: c.warn,
              filter: `saturate(${netTxSaturation})`,
              fontSize: `${Math.max(8, font - 1)}px`,
              transition: 'filter 200ms ease-out',
            }}
          >
            ↑
          </span>
          <span style={{ color: VALUE_COLOR, minWidth: '4ch', fontVariantNumeric: 'tabular-nums' }}>
            {fmtBytes(stats.net_tx)}
          </span>
        </div>
      </div>
    </div>
  );
}

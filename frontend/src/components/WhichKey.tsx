import { Card } from '@astryxdesign/core/Card';
import { Text } from '@astryxdesign/core/Text';
import { KeyCap } from './KeyHint';
import { keyCapKeys, showKeys } from '../lib/nativeEditor/keyLayer';
import type { PendingKeys } from '../lib/nativeEditor/sequences';
import { POPUP_GLASS } from '../lib/nativeEditor/theme';

/** What can follow the keys pending in the native editor, after which-key.nvim. */
export function WhichKey({ pending }: { pending: PendingKeys }) {
  return (
    <div role="status" aria-label={`Keys after ${showKeys(pending.keys)}`}>
      <Card padding={2} elevation="med" width="100%" className="pointer-events-auto" style={POPUP_GLASS}>
        <div className="grid gap-x-6 gap-y-1" style={{ gridTemplateColumns: 'repeat(auto-fill, minmax(16rem, 1fr))' }}>
          {pending.rows.map((row) => (
            <div key={row.key} className="flex min-w-0 items-center gap-2" data-which-key={row.key}>
              <span className="btm-mono flex w-12 flex-none justify-end">
                <KeyCap keys={keyCapKeys(row.key)} />
              </span>
              <Text size="sm" color={row.group ? 'accent' : 'primary'} maxLines={1}>
                {row.group ? `+${row.label}` : row.label}
              </Text>
            </div>
          ))}
        </div>
      </Card>
    </div>
  );
}

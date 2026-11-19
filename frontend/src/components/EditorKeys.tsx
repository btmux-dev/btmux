import { Fragment } from 'react';
import { HStack } from '@astryxdesign/core/Layout';
import { Text } from '@astryxdesign/core/Text';
import { KeyCap } from './KeyHint';
import type { EditorKey } from '../lib/nativeEditor/keyHelp';

/** The native editor's keys for one thing: alternatives, each a sequence of caps. */
export function EditorKeys({ keys }: { keys: EditorKey[] }) {
  return (
    <HStack
      gap={2}
      vAlign="center"
      className="btm-mono"
      data-editor-keys={keys.map((key) => key.caps.join(' ') + (key.mode ? ` (${key.mode})` : '')).join(' | ')}
    >
      {keys.map((key, i) => (
        <Fragment key={i}>
          {i > 0 && (
            <Text size="sm" color="secondary">
              or
            </Text>
          )}
          {/* Wider than a chord's own gap, so `⌘K ⌘I` reads as two steps. */}
          <HStack as="span" gap={3} vAlign="center" className="flex-none">
            {key.caps.map((cap, j) => (
              <KeyCap key={j} keys={cap} />
            ))}
          </HStack>
          {key.mode && (
            <Text size="sm" color="secondary">
              ({key.mode})
            </Text>
          )}
        </Fragment>
      ))}
    </HStack>
  );
}

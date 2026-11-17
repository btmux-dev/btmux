import { Card } from '@astryxdesign/core/Card';
import { HStack, VStack } from '@astryxdesign/core/Layout';
import { ProgressBar } from '@astryxdesign/core/ProgressBar';
import { StatusDot } from '@astryxdesign/core/StatusDot';
import { Text } from '@astryxdesign/core/Text';
import { Spinner } from './Spinner';
import type { Progress } from '../lib/progress';

/** A progress toast: Neovim's progress-messages, native editor language server work. Its container sets the room. */
export function ProgressCard({ progress }: { progress: Progress }) {
  const title = progress.title || progress.source;
  return (
    <Card padding={2} elevation="med" width="min(40ch, 100%)" className="pointer-events-auto">
      <VStack gap={1}>
        <HStack gap={2} vAlign="center">
          {progress.status === 'running' ? (
            <Spinner size="sm" aria-label={title} />
          ) : (
            <StatusDot
              label={progress.status}
              variant={progress.status === 'success' ? 'success' : progress.status === 'failed' ? 'error' : 'neutral'}
            />
          )}
          <Text weight="semibold" maxLines={1} className="min-w-0 flex-1">
            {title}
          </Text>
          {progress.title && progress.source && (
            <Text size="sm" color="secondary">
              {progress.source}
            </Text>
          )}
        </HStack>
        {progress.text && (
          <Text size="sm" color="secondary" maxLines={1}>
            {progress.text}
          </Text>
        )}
        {progress.percent !== null && progress.status === 'running' && (
          <ProgressBar label={title} isLabelHidden value={progress.percent} />
        )}
      </VStack>
    </Card>
  );
}

import { useEffect, type ReactNode } from 'react';
import { useToast } from '@astryxdesign/core/Toast';
import { Button } from '@astryxdesign/core/Button';
import { StatusDot } from '@astryxdesign/core/StatusDot';
import { Text } from '@astryxdesign/core/Text';
import { HStack, VStack } from '@astryxdesign/core/Layout';
import { Link } from '@astryxdesign/core/Link';
import { subscribeNotifications } from '../lib/notifications';
import { findUrls } from '../lib/urlDetect';
import { useStore } from '../state/store';

/** Text with its HTTP(S) URLs as links. */
function linkify(text: string): ReactNode {
  const urls = findUrls(text);
  if (urls.length === 0) return text;
  const parts: ReactNode[] = [];
  let at = 0;
  for (const { url, start, end } of urls) {
    parts.push(
      text.slice(at, start),
      <Link key={start} href={url} target="_blank" className="break-all">
        {url}
      </Link>,
    );
    at = end;
  }
  parts.push(text.slice(at));
  return parts;
}

export function NotificationToasts() {
  const toast = useToast();
  useEffect(() => {
    // Dismissers of shown toasts with an id; an overwrite replaces its entry.
    const byId = new Map<string, () => void>();
    return subscribeNotifications({
      dismiss: (id) => byId.get(id)?.(),
      show: ({ message, level, body, paneId, persistent, id, action }) => {
        const dismiss: () => void = toast({
          type: level === 'error' ? 'error' : 'info',
          ...(persistent && { isAutoHide: false }),
          ...(id && {
            uniqueID: id,
            onHide: () => {
              if (byId.get(id) === dismiss) byId.delete(id);
            },
          }),
          body: (
            <VStack gap={1}>
              <HStack gap={2} align="start">
                {/* Center the dot on the first title line. */}
                <HStack align="center" className="h-lh shrink-0">
                  <StatusDot
                    label={level}
                    variant={level === 'attention' ? 'warning' : level === 'info' ? 'accent' : level}
                  />
                </HStack>
                <Text weight="semibold">{linkify(message)}</Text>
              </HStack>
              {body && <Text>{linkify(body)}</Text>}
            </VStack>
          ),
          endContent: action ? (
            <Button label={action.label} variant="ghost" onClick={() => action.run(dismiss)} />
          ) : paneId ? (
            <Button
              label="View pane"
              variant="ghost"
              onClick={() => {
                dismiss();
                useStore.getState().navigateToPane(paneId);
              }}
            />
          ) : undefined,
        });
        if (id) byId.set(id, dismiss);
      },
    });
  }, [toast]);
  return null;
}

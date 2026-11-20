import { useEffect, useState } from 'react';
import type { ConfigPreview, ConfigUpdate } from '../generated/protocol';

const DEBOUNCE_MS = 150;

type Result = { key: string; revision: unknown; preview?: ConfigPreview; error?: string };

export interface ConfigPreviewResult {
  /** The last preview, possibly for an earlier `update`. */
  preview: ConfigPreview | null;
  error: string | null;
  /** Whether `preview` and `error` are for an earlier `update` or `revision`. */
  isStale: boolean;
}

/**
 * config.toml as saving `update` would write it (the file plus the server's
 * overrides plus `update`), and as Nix, while `enabled`. A new `revision`
 * (a pushed config) asks again.
 */
export function useConfigPreview(update: ConfigUpdate, enabled: boolean, revision: unknown): ConfigPreviewResult {
  const key = JSON.stringify(update);
  const [result, setResult] = useState<Result | null>(null);
  useEffect(() => {
    if (!enabled) return;
    const controller = new AbortController();
    const timer = setTimeout(() => {
      fetch('/api/config/preview', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: key,
        signal: controller.signal,
        cache: 'no-store',
      })
        .then(async (response) => {
          if (!response.ok) {
            throw new Error((await response.text()) || `Could not preview the config (HTTP ${response.status})`);
          }
          return response.json() as Promise<ConfigPreview>;
        })
        .then((preview) => setResult({ key, revision, preview }))
        .catch((error: Error) => {
          if (!controller.signal.aborted) setResult({ key, revision, error: error.message });
        });
    }, DEBOUNCE_MS);
    return () => {
      clearTimeout(timer);
      controller.abort();
    };
  }, [key, enabled, revision]);
  return {
    preview: result?.preview ?? null,
    error: result?.error ?? null,
    isStale: result?.key !== key || result.revision !== revision,
  };
}

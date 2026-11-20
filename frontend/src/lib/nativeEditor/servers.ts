// A view's language servers. lsp-client's plugin, one per view, serves the
// first; the others get plugins of their own here, which features ask too.
import { LSPPlugin, type LSPClient } from '@codemirror/lsp-client';
import { ViewPlugin, type EditorView, type ViewUpdate } from '@codemirror/view';

export type Attach = { client: LSPClient; uri: string; languageID: string };

// Not in lsp-client's types, but what `LSPClient.plugin` creates.
type PluginInternals = LSPPlugin & { update(u: ViewUpdate): void; destroy(): void };
const Plugin = LSPPlugin as unknown as new (view: EditorView, attach: Attach) => PluginInternals;

const others = ViewPlugin.fromClass(
  class {
    readonly plugins: PluginInternals[];
    constructor(view: EditorView, attach: readonly Attach[]) {
      this.plugins = attach.map((a) => new Plugin(view, a));
    }
    update(u: ViewUpdate) {
      for (const p of this.plugins) p.update(u);
    }
    destroy() {
      for (const p of this.plugins) p.destroy();
    }
  },
);

/** Plugins for the servers after the first. */
export const otherServers = (attach: readonly Attach[]) => others.of(attach);

/** Every server's plugin on `view`, the first server's first. */
export function plugins(view: EditorView): LSPPlugin[] {
  const first = LSPPlugin.get(view);
  return first ? [first, ...(view.plugin(others)?.plugins ?? [])] : [];
}

export const has = (p: LSPPlugin, capability: string) =>
  !!(p.client.serverCapabilities as Record<string, unknown> | null)?.[capability];

/** The first server that can do `capability`. */
export const pluginFor = (view: EditorView, capability: string) =>
  plugins(view).find((p) => has(p, capability)) ?? null;

/** Every server that can do `capability`. */
export const pluginsFor = (view: EditorView, capability: string) => plugins(view).filter((p) => has(p, capability));

/** `client`'s plugin on `view`. */
export const pluginOf = (view: EditorView, client: LSPClient) => plugins(view).find((p) => p.client === client) ?? null;

/** Server ids by client: edits and commands go back to the server they came from. */
export const serverIds = new WeakMap<LSPClient, number>();

/** Whether a server can do `capability`, or may once it has started. */
export const mayHave = (view: EditorView, capability: string) =>
  plugins(view).some((p) => p.client.serverCapabilities === null || has(p, capability));

/** Longest wait for servers still starting before a command picks among those started. */
const START_WAIT_MS = 5000;

/** Resolves once `view`'s servers have started, failed, or had `START_WAIT_MS`. */
function started(view: EditorView) {
  const pending = plugins(view).filter((p) => p.client.serverCapabilities === null);
  if (!pending.length) return Promise.resolve();
  return Promise.race([
    Promise.all(pending.map((p) => p.client.initializing.catch(() => null))),
    new Promise((resolve) => setTimeout(resolve, START_WAIT_MS)),
  ]).then(() => undefined);
}

/** `pluginFor`, once the servers have started: a command run while they start waits for them. */
export const pluginReady = (view: EditorView, capability: string) =>
  started(view).then(() => pluginFor(view, capability));

/** `pluginsFor`, once the servers have started. */
export const pluginsReady = (view: EditorView, capability: string) =>
  started(view).then(() => pluginsFor(view, capability));

/**
 * The results of `tasks` once all settle, or `grace` ms after the first does:
 * a slow server doesn't hold back the others' (it gets `undefined`).
 */
export function gather<T>(tasks: Promise<T>[], grace = 1000): Promise<(T | undefined)[]> {
  return new Promise((resolve) => {
    const results: (T | undefined)[] = new Array(tasks.length).fill(undefined);
    let left = tasks.length;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const done = () => {
      clearTimeout(timer);
      resolve(results.slice());
    };
    if (!left) return done();
    tasks.forEach((task, i) =>
      task
        .then(
          (result) => void (results[i] = result),
          () => {},
        )
        .finally(() => {
          if (--left === 0) done();
          else timer ??= setTimeout(done, grace);
        }),
    );
  });
}

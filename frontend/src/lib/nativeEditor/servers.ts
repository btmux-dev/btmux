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

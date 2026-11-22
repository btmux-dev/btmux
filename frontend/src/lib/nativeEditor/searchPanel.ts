import {
  closeSearchPanel,
  findNext,
  findPrevious,
  getSearchQuery,
  replaceAll,
  replaceNext,
  search,
  SearchQuery,
  selectMatches,
  setSearchQuery,
} from '@codemirror/search';
import { runScopeHandlers, type EditorView } from '@codemirror/view';

export interface SearchChange {
  search?: string;
  replace?: string;
  caseSensitive?: boolean;
  regexp?: boolean;
  wholeWord?: boolean;
}

/**
 * The open find and replace panel: `NativeEditor` renders `EditorSearch` into
 * `dom`. Its commands are here, so the component stays out of the lazy
 * CodeMirror chunk.
 */
export interface SearchPanelHandle {
  dom: HTMLElement;
  query(): SearchQuery;
  /** Calls `listener` when the query changes, here or from a command (Mod-f on a selection). */
  subscribe(listener: () => void): () => void;
  set(change: SearchChange): void;
  next(): void;
  previous(): void;
  selectAll(): void;
  replace(): void;
  replaceAll(): void;
  close(): void;
  /** Runs the editor's panel keys (Esc, Mod-f, Mod-g, F3); true if one ran. */
  key(event: KeyboardEvent): boolean;
}

/** CodeMirror's search, with a panel `show` fills: called with the panel on mount, `open` false on destroy. */
export function searchPanel(show: (handle: SearchPanelHandle, open: boolean) => void) {
  return search({
    top: true,
    createPanel(view: EditorView) {
      const dom = document.createElement('div');
      dom.className = 'cm-btmux-search';
      const listeners = new Set<() => void>();
      const handle: SearchPanelHandle = {
        dom,
        query: () => getSearchQuery(view.state),
        subscribe(listener) {
          listeners.add(listener);
          return () => listeners.delete(listener);
        },
        set(change) {
          const query = getSearchQuery(view.state);
          const next = new SearchQuery({
            search: query.search,
            replace: query.replace,
            caseSensitive: query.caseSensitive,
            regexp: query.regexp,
            wholeWord: query.wholeWord,
            literal: query.literal,
            ...change,
          });
          if (!next.eq(query)) view.dispatch({ effects: setSearchQuery.of(next) });
        },
        next: () => void findNext(view),
        previous: () => void findPrevious(view),
        selectAll: () => void selectMatches(view),
        replace: () => void replaceNext(view),
        replaceAll: () => void replaceAll(view),
        close: () => void closeSearchPanel(view),
        key: (event) => runScopeHandlers(view, event, 'search-panel'),
      };
      return {
        dom,
        top: true,
        mount: () => show(handle, true),
        update(update) {
          if (update.transactions.some((tr) => tr.effects.some((e) => e.is(setSearchQuery))))
            for (const listener of listeners) listener();
        },
        destroy: () => show(handle, false),
      };
    },
  });
}

import {
  ChangeSet,
  EditorSelection,
  EditorState,
  type Extension,
  type Transaction,
  type TransactionSpec,
} from '@codemirror/state';
import type { EditorView } from '@codemirror/view';
import type { StateEffect } from '@codemirror/state';
import { collab, getSyncedVersion, receiveUpdates, sendableUpdates } from '@codemirror/collab';
import type { BufferInfo, DiskStatus, LineEnding, Update } from '../../generated/protocol';
import { editorConnection, type BufferListener } from './connection';

/**
 * Which of `count` updates starting at `version` are new to an editor synced
 * to `synced`: the index of the first, or `gap` if some are missing.
 */
export function freshFrom(synced: number, version: number): number | 'gap' {
  return version > synced ? 'gap' : synced - version;
}

export interface DocStatus {
  /** The buffer is loaded and connected. */
  ready: boolean;
  error: string | null;
  dirty: boolean;
  disk: DiskStatus;
  lineEnding: LineEnding;
}

const randomId = () => Math.random().toString(36).slice(2, 12);

/**
 * One editor's copy of a server buffer, kept in sync through
 * `@codemirror/collab`. Its state lives in a view while shown, else here.
 */
export class Doc implements BufferListener {
  buffer: string | null = null;
  view: EditorView | null = null;
  status: DocStatus = { ready: false, error: null, dirty: false, disk: 'same', lineEnding: 'lf' };
  private ownState: EditorState | null = null;
  /** Where the view was scrolled when this doc was last shown. */
  private scroll: StateEffect<unknown> | null = null;
  private pushing = false;
  private readonly clientID = randomId();

  constructor(
    readonly path: string,
    private readonly extensions: (doc: Doc, text: string) => Extension,
    private readonly onStatus: () => void,
  ) {}

  get state(): EditorState | null {
    return this.view?.state ?? this.ownState;
  }

  open() {
    editorConnection.open(this);
  }

  close() {
    editorConnection.close(this);
  }

  /** Show in `view`, or take the state back from it (`null`). Returns false if not loaded yet. */
  attach(view: EditorView | null): boolean {
    if (view === this.view) return true;
    if (this.view) {
      this.ownState = this.view.state;
      this.scroll = this.view.scrollSnapshot();
      this.view = null;
    }
    if (!view) return true;
    if (!this.ownState) return false;
    this.view = view;
    view.setState(this.ownState);
    this.ownState = null;
    if (this.scroll) view.dispatch({ effects: this.scroll });
    return true;
  }

  /** Change the state, shown or not. */
  update(spec: TransactionSpec) {
    const state = this.state;
    if (state) this.apply(state.update(spec));
  }

  private setStatus(status: Partial<DocStatus>) {
    this.status = { ...this.status, ...status };
    this.onStatus();
  }

  resume() {
    const state = this.state;
    return this.buffer && state ? { buffer: this.buffer, version: getSyncedVersion(state) } : null;
  }

  opened(info: BufferInfo) {
    const old = this.state;
    const head = Math.min(old?.selection.main.head ?? 0, info.text.length);
    const state = EditorState.create({
      doc: info.text,
      selection: EditorSelection.single(head),
      extensions: [this.extensions(this, info.text), collab({ startVersion: info.version, clientID: this.clientID })],
    });
    this.buffer = info.id;
    this.pushing = false;
    if (this.view) this.view.setState(state);
    else this.ownState = state;
    this.setStatus({ ready: true, error: null, dirty: info.dirty, disk: info.disk, lineEnding: info.line_ending });
  }

  resumed() {
    this.pushing = false;
    this.setStatus({ ready: true, error: null });
  }

  updates(version: number, updates: Update[]) {
    const state = this.state;
    if (!state) return;
    const from = freshFrom(getSyncedVersion(state), version);
    if (from === 'gap') {
      editorConnection.reopen(this);
      return;
    }
    const fresh = updates.slice(from);
    if (fresh.length) {
      this.apply(
        receiveUpdates(
          state,
          fresh.map((u) => ({ changes: ChangeSet.fromJSON(u.changes), clientID: u.client_id })),
        ),
      );
    }
    // Ours came back (accepted), or others' did (ours was stale): push what's left.
    this.pushing = false;
    this.push();
  }

  private apply(tr: Transaction) {
    if (this.view) this.view.dispatch(tr);
    else this.ownState = tr.state;
  }

  bufferState(dirty: boolean, disk: DiskStatus) {
    this.setStatus({ dirty, disk });
  }

  failed(error: string) {
    this.setStatus({ error, ready: false });
  }

  disconnected() {
    this.pushing = false;
    this.setStatus({ ready: false });
  }

  /** Send local changes, one batch in flight at a time. */
  push() {
    const state = this.state;
    if (this.pushing || !this.buffer || !state || !this.status.ready) return;
    const sendable = sendableUpdates(state);
    if (!sendable.length) return;
    const updates = sendable.map((u) => ({ client_id: u.clientID, changes: u.changes.toJSON() }));
    if (editorConnection.push(this.buffer, getSyncedVersion(state), updates)) this.pushing = true;
  }
}

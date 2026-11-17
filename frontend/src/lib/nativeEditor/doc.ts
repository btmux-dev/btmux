import {
  ChangeSet,
  EditorSelection,
  EditorState,
  type Extension,
  type Text,
  type Transaction,
  type TransactionSpec,
} from '@codemirror/state';
import type { EditorView } from '@codemirror/view';
import type { StateEffect } from '@codemirror/state';
import { collab, getSyncedVersion, receiveUpdates, sendableUpdates } from '@codemirror/collab';
import type { BufferInfo, DiskStatus, LineEnding, Update } from '../../generated/protocol';
import { useStore } from '../../state/store';
import { editorConnection, type BufferListener } from './connection';
import { rebase } from './rebase';

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

/** The doc each view shows. */
export const viewDocs = new WeakMap<EditorView, Doc>();

/**
 * One editor's copy of a server buffer, kept in sync through
 * `@codemirror/collab`. Its state lives in a view while shown, else here.
 */
export class Doc implements BufferListener {
  buffer: string | null = null;
  view: EditorView | null = null;
  languageId: string | null = null;
  uri: string | null = null;
  /** Language server ids, from the server. */
  servers: number[] = [];
  status: DocStatus = { ready: false, error: null, dirty: false, disk: 'same', lineEnding: 'lf' };
  private ownState: EditorState | null = null;
  /** The document at the synced version: what unconfirmed changes apply to. */
  private synced: Text | null = null;
  /** Where the view was scrolled when this doc was last shown. */
  private scroll: StateEffect<unknown> | null = null;
  /** Updates already pushed; all unconfirmed ones go with each push, and the server skips repeats. */
  private sent = new WeakSet<object>();
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
      if (viewDocs.get(this.view) === this) viewDocs.delete(this.view);
      this.view = null;
    }
    if (!view) return true;
    if (!this.ownState) return false;
    this.view = view;
    viewDocs.set(view, this);
    view.setState(this.ownState);
    this.ownState = null;
    if (this.scroll) view.dispatch({ effects: this.scroll });
    return true;
  }

  /** Change the state, shown or not, sending any edit. */
  update(spec: TransactionSpec) {
    const state = this.state;
    if (!state) return;
    this.apply(state.update(spec));
    this.push();
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
    const base = this.synced;
    // Changes the server never got (typed while disconnected): they go on
    // top of its text rather than being lost.
    const unconfirmed = old ? sendableUpdates(old) : [];
    const local =
      base && unconfirmed.length
        ? unconfirmed.reduce((set, u) => set.compose(u.changes), ChangeSet.empty(base.length))
        : null;
    const head = Math.min(old?.selection.main.head ?? 0, info.text.length);
    // Before the state: its extensions read them.
    this.buffer = info.id;
    this.languageId = info.language_id;
    this.uri = info.uri;
    this.servers = info.servers;
    const state = EditorState.create({
      doc: info.text,
      selection: EditorSelection.single(head),
      extensions: [this.extensions(this, info.text), collab({ startVersion: info.version, clientID: this.clientID })],
    });
    this.synced = state.doc;
    this.sent = new WeakSet();
    if (this.view) this.view.setState(state);
    else this.ownState = state;
    this.setStatus({ ready: true, error: null, dirty: info.dirty, disk: info.disk, lineEnding: info.line_ending });
    // Already there: the server applied them before the connection dropped.
    if (!old || !base || !local || old.doc.eq(state.doc)) return;
    const merged = rebase(base, local, state.doc);
    this.update({
      changes: merged.changes,
      selection: EditorSelection.single(merged.after.mapPos(old.selection.main.head)),
      userEvent: 'input',
    });
    if (!merged.exact)
      useStore.getState().showToast('Merged edits made while disconnected', 'attention', {
        body: `${this.path} changed meanwhile; check the result.`,
      });
  }

  resumed() {
    this.sent = new WeakSet();
    this.setStatus({ ready: true, error: null });
    this.push();
  }

  updates(version: number, updates: Update[]) {
    const state = this.state;
    if (!state) return;
    const from = freshFrom(getSyncedVersion(state), version);
    if (from === 'gap') {
      editorConnection.reopen(this);
      return;
    }
    const fresh = updates.slice(from).map((u) => ({ changes: ChangeSet.fromJSON(u.changes), clientID: u.client_id }));
    if (fresh.length) {
      if (this.synced) this.synced = fresh.reduce((doc, u) => u.changes.apply(doc), this.synced);
      this.apply(receiveUpdates(state, fresh));
    }
    // Others' changes rebase ours, which then go again.
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

  /** Edits go on meanwhile; a resume or a reopen (`opened`) sends them. */
  disconnected() {
    this.setStatus({ ready: false });
  }

  /** Send local changes not sent yet, with the unconfirmed ones before them. */
  push() {
    const state = this.state;
    if (!this.buffer || !state || !this.status.ready) return;
    const sendable = sendableUpdates(state);
    if (sendable.every((u) => this.sent.has(u))) return;
    const updates = sendable.map((u) => ({ client_id: u.clientID, changes: u.changes.toJSON() }));
    if (editorConnection.push(this.buffer, getSyncedVersion(state), updates))
      for (const u of sendable) this.sent.add(u);
  }

  /** Some local changes haven't been sent (disconnected or reopening). */
  unsent(): boolean {
    const state = this.state;
    return !!state && sendableUpdates(state).some((u) => !this.sent.has(u));
  }

  /** The server's version once it has every change pushed: what language server positions refer to. */
  version(): number | null {
    const state = this.state;
    return state ? getSyncedVersion(state) + sendableUpdates(state).length : null;
  }
}

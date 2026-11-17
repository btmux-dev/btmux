/** Work shown as a progress card: Neovim progress-messages, language server `$/progress`. */
export interface Progress {
  id: string;
  text: string;
  title: string;
  source: string;
  status: 'running' | 'success' | 'failed' | 'cancel';
  percent: number | null;
  /** Shortly after finishing, or once stale. */
  expires: number;
}

/** How long a finished card stays. */
export const PROGRESS_DONE_MS = 1500;

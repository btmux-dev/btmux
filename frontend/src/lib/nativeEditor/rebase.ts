import { ChangeSet, type ChangeSpec, type Text } from '@codemirror/state';

/** Most line edits a diff looks for before replacing the changed region whole. */
const DIFF_LIMIT = 500;

/** A document's lines, each with its line break. */
const tokens = (doc: Text) => {
  const lines = doc.toJSON();
  return lines.map((line, i) => (i < lines.length - 1 ? `${line}\n` : line));
};

/** Pairs of equal tokens in `a` and `b` (Myers), in order; null past `limit` edits. */
function matches(a: string[], b: string[], limit: number): [number, number][] | null {
  const n = a.length;
  const m = b.length;
  const max = Math.min(n + m, limit);
  const off = max + 1;
  const v = new Int32Array(2 * max + 3);
  const trace: Int32Array[] = [];
  for (let d = 0; d <= max; d++) {
    trace.push(v.slice());
    for (let k = -d; k <= d; k += 2) {
      let x = k === -d || (k !== d && v[off + k - 1] < v[off + k + 1]) ? v[off + k + 1] : v[off + k - 1] + 1;
      let y = x - k;
      while (x < n && y < m && a[x] === b[y]) {
        x++;
        y++;
      }
      v[off + k] = x;
      if (x >= n && y >= m) {
        const pairs: [number, number][] = [];
        for (let e = d; e > 0; e--) {
          const prev = trace[e];
          const k = x - y;
          const pk = k === -e || (k !== e && prev[off + k - 1] < prev[off + k + 1]) ? k + 1 : k - 1;
          const px = prev[off + pk];
          const py = px - pk;
          while (x > px && y > py) pairs.push([--x, --y]);
          x = px;
          y = py;
        }
        while (x > 0 && y > 0) pairs.push([--x, --y]);
        return pairs.reverse();
      }
    }
  }
  return null;
}

/** The changes that turn `a` into `b`, line by line. */
export function diff(a: Text, b: Text): ChangeSet {
  const ta = tokens(a);
  const tb = tokens(b);
  let start = 0;
  while (start < ta.length && start < tb.length && ta[start] === tb[start]) start++;
  let endA = ta.length;
  let endB = tb.length;
  while (endA > start && endB > start && ta[endA - 1] === tb[endB - 1]) {
    endA--;
    endB--;
  }
  const midA = ta.slice(start, endA);
  const midB = tb.slice(start, endB);
  const pairs = matches(midA, midB, DIFF_LIMIT) ?? [];
  const offsets = [0];
  for (const t of ta) offsets.push(offsets[offsets.length - 1] + t.length);
  const specs: ChangeSpec[] = [];
  let ai = 0;
  let bi = 0;
  for (const [x, y] of [...pairs, [midA.length, midB.length]]) {
    if (x > ai || y > bi)
      specs.push({ from: offsets[start + ai], to: offsets[start + x], insert: midB.slice(bi, y).join('') });
    ai = x + 1;
    bi = y + 1;
  }
  return ChangeSet.of(specs, a.length);
}

/**
 * Local changes made on `base`, rebased onto `server` (what `base` became
 * meanwhile), as collab rebases concurrent edits. `after` maps positions in
 * the local document to the merged one; `exact` when `server` is `base`.
 */
export function rebase(base: Text, local: ChangeSet, server: Text) {
  const theirs = diff(base, server);
  return { changes: local.map(theirs), after: theirs.map(local, true), exact: theirs.empty };
}

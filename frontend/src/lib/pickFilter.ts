import { matchesCase } from './smartCase';

/** What each item is filtered on: its label and description, as is and lowercased. */
export interface PickHaystack {
  text: string;
  lower: string;
}

export function pickHaystack(item: { label: string; description: string | null }): PickHaystack {
  const text = item.description ? `${item.label}\t${item.description}` : item.label;
  return { text, lower: text.toLowerCase() };
}

/** A label on one row, with whole items' line breaks and tabs visible. */
export function pickLabel(label: string): string {
  return label.replace(/\n/g, '␤').replace(/\t/g, '␉');
}

/**
 * Items containing every whitespace-separated term, each in smart case as in
 * fzf, up to `limit`: those whose label starts with the first term, then the
 * rest, each in input order, or only input order with `keepOrder`. `total`
 * counts every match.
 */
export function filterPick(
  haystacks: PickHaystack[],
  query: string,
  limit: number,
  keepOrder = false,
): { indices: number[]; total: number } {
  const terms = query
    .split(/\s+/)
    .filter(Boolean)
    .map((term) => ({ term, exact: matchesCase(term) }));
  const first: number[] = [];
  const rest: number[] = [];
  haystacks.forEach((haystack, index) => {
    const text = (exact: boolean) => (exact ? haystack.text : haystack.lower);
    if (!terms.every(({ term, exact }) => text(exact).includes(term))) return;
    (terms.length && !keepOrder && text(terms[0].exact).startsWith(terms[0].term) ? first : rest).push(index);
  });
  return { indices: [...first, ...rest].slice(0, limit), total: first.length + rest.length };
}

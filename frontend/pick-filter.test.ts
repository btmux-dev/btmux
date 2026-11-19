import { expect, test } from 'bun:test';
import { filterPick, pickHaystack } from './src/lib/pickFilter';

const haystacks = (labels: string[]) => labels.map((label) => pickHaystack({ label, description: null }));
const items = haystacks(['checkout', 'cherry-pick', 'rebase', 'switch']);

test('an empty query keeps input order', () => {
  expect(filterPick(items, '  ', 10)).toEqual({ indices: [0, 1, 2, 3], total: 4 });
});

test('label prefixes rank first', () => {
  expect(filterPick(haystacks(['a-che', 'che-b', 'c-che']), 'che', 10).indices).toEqual([1, 0, 2]);
});

test('keeping order ignores prefixes', () => {
  const ordered = haystacks(['echo git-newer', 'git status']);
  expect(filterPick(ordered, 'git', 10, true).indices).toEqual([0, 1]);
  expect(filterPick(ordered, 'git', 1, true)).toEqual({ indices: [0], total: 2 });
});

test('every term must match, each in smart case', () => {
  expect(filterPick(items, 'che pick', 10).indices).toEqual([1]);
  expect(filterPick(items, 'che zzz', 10).total).toBe(0);
  const mixed = haystacks(['Checkout', 'checkout']);
  expect(filterPick(mixed, 'check', 10).indices).toEqual([0, 1]);
  expect(filterPick(mixed, 'Check', 10).indices).toEqual([0]);
  expect(filterPick(mixed, 'Check out', 10).indices).toEqual([0]);
});

test('descriptions match but do not count as prefixes', () => {
  const described = [pickHaystack({ label: 'x', description: 'Note' }), pickHaystack({ label: 'note', description: null })];
  expect(filterPick(described, 'note', 10).indices).toEqual([1, 0]);
});

test('the limit keeps the total', () => {
  expect(filterPick(items, '', 2)).toEqual({ indices: [0, 1], total: 4 });
});

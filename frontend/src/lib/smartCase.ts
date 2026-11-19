/** Smart case, as in Vim and fzf: a query with an uppercase letter matches case. */
export function matchesCase(query: string): boolean {
  return query !== query.toLowerCase();
}

/** Whether a text contains `query`, in smart case. */
export function smartCase(query: string): (text: string) => boolean {
  if (matchesCase(query)) return (text) => text.includes(query);
  return (text) => text.toLowerCase().includes(query);
}

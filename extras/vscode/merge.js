// Three-way merges of the shared user files into VS Code's own: a change made
// in btmux's VS Code stays until the shared file changes the same setting or
// binding. `base` is the shared file as last applied, if any.

/** Parse JSON with comments and trailing commas, as VS Code's user files allow. */
function parseJsonc(text) {
  // Strings are copied whole, so neither pass looks inside them.
  const scan = (input, other) => {
    let output = '';
    for (let i = 0; i < input.length; i++) {
      if (input[i] === '"') {
        const start = i;
        for (i++; i < input.length && input[i] !== '"'; i++) if (input[i] === '\\') i++;
        output += input.slice(start, i + 1);
      } else {
        const [skip, add] = other(input, i);
        output += add;
        i += skip;
      }
    }
    return output;
  };
  const plain = scan(text.replace(/^\uFEFF/, ''), (input, i) => {
    if (input.startsWith('//', i)) {
      const end = input.indexOf('\n', i);
      return [(end < 0 ? input.length : end) - i - 1, ''];
    }
    if (input.startsWith('/*', i)) {
      const end = input.indexOf('*/', i + 2);
      return [(end < 0 ? input.length : end + 2) - i - 1, ' '];
    }
    return [0, input[i]];
  });
  const json = scan(plain, (input, i) => [0, input[i] === ',' && /^\s*[}\]]/.test(input.slice(i + 1)) ? '' : input[i]]);
  return JSON.parse(json);
}

const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);

/**
 * `ours` with the settings `theirs` changed since `base`, except those btmux
 * `written`, which stay so they don't flicker. Each of `groups`, a sync
 * setting and the keys it writes, that the result turns off gets `theirs`'
 * values for those keys back instead.
 */
function mergeSettings(base, theirs, ours, written = new Set(), groups = []) {
  const result = { ...ours };
  const apply = (key) => {
    if (key in theirs) result[key] = theirs[key];
    else delete result[key];
  };
  for (const key of new Set([...Object.keys(base ?? {}), ...Object.keys(theirs)])) {
    if (!written.has(key) && !(base && same(base[key], theirs[key]))) apply(key);
  }
  for (const [setting, keys] of groups) {
    if (result[setting] === false) keys.filter((key) => written.has(key)).forEach(apply);
  }
  return result;
}

/**
 * `theirs`, in its order (VS Code's last matching binding wins), without the
 * bindings removed here and with those added here, each before the shared
 * binding it preceded. Unchanged since `base`, it leaves `ours` as it is.
 */
function mergeKeybindings(base, theirs, ours) {
  if (base && same(base, theirs)) return ours;
  const id = (binding) => JSON.stringify(binding);
  const before = new Set((base ?? []).map(id));
  const after = new Set(theirs.map(id));
  const present = new Set(ours.map(id));
  const shared = theirs.filter((binding) => !before.has(id(binding)) || present.has(id(binding)));
  const kept = new Set(shared.map(id));
  // Bindings added here, by the shared binding they precede.
  const added = new Map();
  let pending = [];
  for (const binding of ours) {
    const key = id(binding);
    if (kept.has(key)) {
      if (pending.length) added.set(key, [...(added.get(key) ?? []), ...pending]);
      pending = [];
    } else if (!before.has(key) && !after.has(key)) {
      pending.push(binding);
    }
  }
  const result = [];
  for (const binding of shared) {
    result.push(...(added.get(id(binding)) ?? []), binding);
    added.delete(id(binding));
  }
  return [...result, ...pending];
}

module.exports = { parseJsonc, mergeSettings, mergeKeybindings, same };

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

/** `ours` with the settings `theirs` changed since `base`, except `keep`'s. */
function mergeSettings(base, theirs, ours, keep) {
  const result = { ...ours };
  for (const key of new Set([...Object.keys(base ?? {}), ...Object.keys(theirs)])) {
    if (keep.has(key) || (base && same(base[key], theirs[key]))) continue;
    if (key in theirs) result[key] = theirs[key];
    else delete result[key];
  }
  return result;
}

/** `ours` without the bindings `theirs` dropped since `base`, and with those it added. */
function mergeKeybindings(base, theirs, ours) {
  const id = (binding) => JSON.stringify(binding);
  const before = new Set((base ?? []).map(id));
  const after = new Set(theirs.map(id));
  const result = ours.filter((binding) => !before.has(id(binding)) || after.has(id(binding)));
  const present = new Set(result.map(id));
  for (const binding of theirs) {
    if (!before.has(id(binding)) && !present.has(id(binding))) result.push(binding);
  }
  return result;
}

module.exports = { parseJsonc, mergeSettings, mergeKeybindings, same };

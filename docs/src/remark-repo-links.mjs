// Rewrites relative links, written to work on GitHub, for the site: pages
// in the content directory become routes, other repo files GitHub URLs.
// Routes, and links already written as one (`/start/`), get the site's base.
import { statSync } from "node:fs";
import { dirname, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { visit } from "unist-util-visit";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const content = resolve(root, "docs", "src", "content", "docs");

export function remarkRepoLinks({ repo, base = "" }) {
  return (tree, file) => {
    if (!file.path) return;
    visit(tree, ["link", "definition"], (node) => {
      if (/^\/(?!\/)/.test(node.url)) node.url = base + node.url;
      if (/^[a-z]+:|^[#/]/.test(node.url)) return;
      const [path, hash] = node.url.split("#");
      const anchor = hash ? `#${hash}` : "";
      const target = resolve(dirname(file.path), path);
      const rel = relative(content, target).replace(/\.mdx$/, ".md");
      if (rel.endsWith(".md") && !rel.startsWith("..")) {
        const slug = rel.slice(0, -3).replace(/(^|\/)index$/, "");
        node.url = `${base}/${slug}${slug ? "/" : ""}${anchor}`;
      } else {
        const kind = statSync(target, { throwIfNoEntry: false })?.isDirectory()
          ? "tree"
          : "blob";
        node.url = `${repo}/${kind}/main/${relative(root, target)}${anchor}`;
      }
    });
  };
}

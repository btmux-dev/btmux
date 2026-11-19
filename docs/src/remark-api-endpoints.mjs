// Replaces a `<!-- api-endpoints -->` comment with the REST API's operations
// from public/openapi.json, a table per tag linking to the API reference, so
// the list is on the page and in llms-full.txt, which skip the reference's
// generated pages.
import { readFileSync } from "node:fs";

const spec = JSON.parse(
  readFileSync(new URL("../public/openapi.json", import.meta.url), "utf8"),
);

const text = (value) => ({ type: "text", value });
const cell = (...children) => ({ type: "tableCell", children });

function tables() {
  const operations = Object.entries(spec.paths).flatMap(([path, item]) =>
    Object.entries(item).map(([method, operation]) => ({
      path,
      method,
      ...operation,
    })),
  );
  return spec.tags.flatMap(({ name }) => {
    const rows = operations
      .filter((operation) => operation.tags.includes(name))
      .map((operation) => ({
        type: "tableRow",
        children: [
          cell({
            type: "inlineCode",
            value: `${operation.method.toUpperCase()} ${operation.path}`,
          }),
          cell({
            type: "link",
            url: `/automation/api/operations/${operation.operationId}/`,
            children: [text(operation.summary)],
          }),
        ],
      }));
    const header = {
      type: "tableRow",
      children: [cell(text("Endpoint")), cell(text("Does"))],
    };
    return [
      { type: "heading", depth: 3, children: [text(name)] },
      { type: "table", align: [null, null], children: [header, ...rows] },
    ];
  });
}

export function remarkApiEndpoints() {
  return (tree) => {
    tree.children = tree.children.flatMap((node) =>
      node.type === "html" && node.value.trim() === "<!-- api-endpoints -->"
        ? tables()
        : [node],
    );
  };
}

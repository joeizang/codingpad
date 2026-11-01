import type { DumpNode } from "./protocol";

export function DumpTree({ node }: { node: DumpNode }) {
  switch (node.kind) {
    case "null":
      return <span className="value-null">None</span>;
    case "primitive":
      return (
        <span className="value-primitive">
          {typeof node.v === "boolean"
            ? node.v
              ? "True"
              : "False"
            : String(node.v)}
        </span>
      );
    case "string":
      return <span className="value-string">{JSON.stringify(node.v)}</span>;
    case "list":
    case "object": {
      const fields =
        node.kind === "list"
          ? node.items.map((value, index) => ({ name: String(index), value }))
          : node.fields;
      return (
        <details open>
          <summary>
            {node.type}{" "}
            <span className="muted">
              ({fields.length}
              {node.truncated ? "+" : ""})
            </span>
          </summary>
          <div className="tree-fields">
            {fields.map(({ name, value }, index) => (
              <div className="tree-field" key={index}>
                <span className="field-name">{name}</span>
                <DumpTree node={value} />
              </div>
            ))}
            {node.truncated && (
              <p className="muted">Preview truncated.</p>
            )}
          </div>
        </details>
      );
    }
  }
}

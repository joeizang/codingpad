import { useEffect, useRef, useState } from "react";
import type { DumpNode } from "./protocol";

export type Expand = (handle: string, cursor: string | null) => Promise<DumpNode>;

type Props = {
  node: DumpNode;
  updates?: Record<string, DumpNode>;
  expand?: Expand;
  onOpenLine?: (line: number) => void;
  onExpandValue?: (node: DumpNode, handle: string) => void;
};

const TABLE_ROW_HEIGHT = 27;
const TABLE_PAGE = 100;

export function DumpTree(props: Props) {
  const { node, updates, expand, onOpenLine, onExpandValue } = props;
  const shown = node.id ? updates?.[node.id] : undefined;
  if (shown && shown !== node) {
    return (
      <DumpTree
        node={shown}
        updates={updates}
        expand={expand}
        onOpenLine={onOpenLine}
      />
    );
  }
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
      return (
        <span className="value-string">
          {JSON.stringify(node.v)}
          {node.truncated && (
            <span className="muted">
              {" "}
              (truncated{node.total !== undefined ? `, ${node.total} chars` : ""})
            </span>
          )}
        </span>
      );
    case "lazy":
      return <LazyNode key={node.handle} {...props} node={node} />;
    case "table":
      return <TableNode key={node.id ?? node.handle} {...props} node={node} />;
    case "image":
      return (
        <figure className="dump-image">
          <img src={node.src} alt={node.name ?? node.mime} />
          <figcaption>{node.name ?? node.mime}</figcaption>
        </figure>
      );
    case "html":
      return (
        <iframe
          className="dump-html"
          sandbox=""
          srcDoc={node.html}
          title="CodingPad HTML output"
        />
      );
    case "live":
      return (
        <div className="dump-live">
          <span className="live-badge">live</span>
          <DumpTree
            key={node.node.id}
            node={node.node}
            updates={updates}
            expand={expand}
            onOpenLine={onOpenLine}
            onExpandValue={onExpandValue}
          />
        </div>
      );
    case "error":
      return <ErrorNode node={node} onOpenLine={onOpenLine} />;
    case "list":
    case "object":
      return (
        <CollectionNode key={node.id ?? "collection"} {...props} node={node} />
      );
  }
}

function CollectionNode({
  node,
  updates,
  expand,
  onOpenLine,
  onExpandValue,
}: Props & { node: Extract<DumpNode, { kind: "list" | "object" }> }) {
  const [items, setItems] = useState<DumpNode[]>(
    node.kind === "list" ? node.items : [],
  );
  const [fields, setFields] = useState(
    node.kind === "object" ? node.fields : [],
  );
  const [next, setNext] = useState<string | null>(node.next ?? null);
  const [handle, setHandle] = useState(node.handle);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string>();
  const expandRef = useRef(expand);
  expandRef.current = expand;

  async function loadMore() {
    if (!handle || !next || loading) return;
    setLoading(true);
    setError(undefined);
    try {
      const page = await expandRef.current?.(handle, next);
      if (!page) return;
      if (page.kind === "list" && node.kind === "list") {
        setItems((current) => [...current, ...page.items]);
      } else if (page.kind === "object" && node.kind === "object") {
        setFields((current) => [...current, ...page.fields]);
      } else {
        throw new Error("Unexpected page from the kernel");
      }
      setNext(page.next ?? null);
      setHandle(page.handle ?? handle);
    } catch (problem) {
      setError(String(problem));
    } finally {
      setLoading(false);
    }
  }

  const rows =
    node.kind === "list"
      ? items.map((value, index) => ({ name: String(index), value }))
      : fields;
  const total = node.total ?? rows.length;
  return (
    <details open>
      <summary>
        {node.type}{" "}
        <span className="muted">
          ({rows.length}
          {next ? ` of ${total}` : ""})
        </span>
      </summary>
      <div className="tree-fields">
        {rows.map(({ name, value }, index) => (
          <div className="tree-field" key={`${name}-${index}`}>
            <span className="field-name">{name}</span>
            <DumpTree
              node={value}
              updates={updates}
              expand={expand}
              onOpenLine={onOpenLine}
              onExpandValue={onExpandValue}
            />
          </div>
        ))}
        {(next || error) && (
          <div className="tree-more">
            <button disabled={loading} onClick={() => void loadMore()}>
              {loading ? "Loading…" : "Show more"}
            </button>
            {error && <span className="value-error">{error}</span>}
          </div>
        )}
      </div>
    </details>
  );
}

function LazyNode({
  node,
  updates,
  expand,
  onOpenLine,
  onExpandValue,
}: Props & { node: Extract<DumpNode, { kind: "lazy" }> }) {
  const [child, setChild] = useState<DumpNode>();
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string>();
  async function reveal() {
    if (loading) return;
    setLoading(true);
    setError(undefined);
    try {
      const value = await expand?.(node.handle, null);
      if (!value) return;
      if (onExpandValue) onExpandValue(value, node.handle);
      else setChild(value);
    } catch (problem) {
      setError(String(problem));
    } finally {
      setLoading(false);
    }
  }
  if (child) {
    return (
      <DumpTree
        node={child}
        updates={updates}
        expand={expand}
        onOpenLine={onOpenLine}
      />
    );
  }
  return (
    <span className="lazy">
      <span className="muted">{node.preview}</span>{" "}
      <button disabled={loading || !expand} onClick={() => void reveal()}>
        {loading ? "Expanding…" : "Expand"}
      </button>
      {error && <span className="value-error">{error}</span>}
    </span>
  );
}

function TableNode({
  node,
  updates,
  expand,
  onOpenLine,
  onExpandValue,
}: Props & { node: Extract<DumpNode, { kind: "table" }> }) {
  const [pages, setPages] = useState<Record<number, DumpNode[][]>>({
    0: node.rows,
  });
  const [start, setStart] = useState(0);
  const [total, setTotal] = useState(node.total);
  const [totalKnown, setTotalKnown] = useState(node.totalKnown !== false);
  const [error, setError] = useState<string>();
  const [expanded, setExpanded] = useState<
    { handle: string; node: DumpNode }[]
  >([]);
  const requested = useRef(new Set<number>([0]));
  const expandRef = useRef(expand);
  expandRef.current = expand;

  const viewportRows = 10;
  const first = Math.max(0, Math.floor(start) - 2);
  const last = Math.min(total, first + viewportRows + 4);

  useEffect(() => {
    if (!node.handle) return;
    for (
      let page = Math.floor(first / TABLE_PAGE) * TABLE_PAGE;
      page < last && page < total;
      page += TABLE_PAGE
    ) {
      if (requested.current.has(page)) continue;
      requested.current.add(page);
      expandRef.current?.(node.handle, String(page))
        .then((result) => {
          if (result.kind !== "table") {
            throw new Error("Unexpected table page from the kernel");
          }
          setPages((current) => ({ ...current, [page]: result.rows }));
          if (node.totalKnown === false) {
            setTotal(result.total);
            setTotalKnown(result.totalKnown !== false);
          }
        })
        .catch((problem) => {
          requested.current.delete(page);
          setError(String(problem));
        });
    }
  }, [first, last, node.handle, node.totalKnown, total]);

  const rowCount = Math.max(1, Math.min(total, viewportRows));
  const rowAt = (index: number) => {
    const page = Math.floor(index / TABLE_PAGE) * TABLE_PAGE;
    return pages[page]?.[index - page];
  };
  const rows = [];
  for (let index = first; index < last; index++) {
    rows.push({ index, cells: rowAt(index) });
  }
  const width = Math.max(node.columns.length, 1) * 140;
  return (
    <div className="table" role="table" aria-label="Table">
      <div
        className="table-head"
        role="row"
        style={{ gridTemplateColumns: `repeat(${node.columns.length}, minmax(140px, 1fr))`, minWidth: width }}
      >
        {node.columns.map((column) => (
          <span role="columnheader" key={column.name}>
            {column.name}
          </span>
        ))}
      </div>
      <div
        className="table-viewport"
        style={{ height: rowCount * TABLE_ROW_HEIGHT }}
        onScroll={(event) =>
          setStart(event.currentTarget.scrollTop / TABLE_ROW_HEIGHT)
        }
      >
        <div
          className="table-inner"
          style={{ height: total * TABLE_ROW_HEIGHT, minWidth: width }}
        >
          {rows.map(({ index, cells }) => (
            <div
              className="table-row"
              role="row"
              key={index}
              style={{
                top: index * TABLE_ROW_HEIGHT,
                gridTemplateColumns: `repeat(${node.columns.length}, minmax(140px, 1fr))`,
              }}
            >
              {node.columns.map((column, columnIndex) => (
                <span className="table-cell" role="cell" key={column.name}>
                  {cells ? (
                    <DumpTree
                      node={cells[columnIndex] ?? { kind: "null" }}
                      updates={updates}
                      expand={expand}
                      onOpenLine={onOpenLine}
                      onExpandValue={(value, handle) =>
                        setExpanded((current) =>
                          current.some((item) => item.handle === handle)
                            ? current
                            : [...current, { handle, node: value }],
                        )
                      }
                    />
                  ) : (
                    <span className="muted">…</span>
                  )}
                </span>
              ))}
            </div>
          ))}
        </div>
      </div>
      <div className="table-footer">
        <span className="muted">
          {totalKnown ? total : Math.max(0, total - 1)}{totalKnown ? "" : "+"} rows
          {node.truncated && " (results truncated)"}
        </span>
        {error && <span className="value-error">{error}</span>}
      </div>
      {expanded.length > 0 && (
        <div className="table-expanded">
          {expanded.map(({ handle, node: value }) => (
            <div key={handle}>
              <span className="muted">Expanded cell</span>
              <DumpTree
                node={value}
                updates={updates}
                expand={expand}
                onOpenLine={onOpenLine}
                onExpandValue={onExpandValue}
              />
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function ErrorNode({
  node,
  onOpenLine,
}: {
  node: Extract<DumpNode, { kind: "error" }>;
  onOpenLine?: (line: number) => void;
}) {
  return (
    <div className="dump-error" role="alert">
      <div className="error-title">
        <strong>{node.type}</strong>: {node.message}
      </div>
      {node.causes && node.causes.length > 0 && (
        <div className="error-causes">
          {node.causes.map((cause, index) => (
            <div key={index} className="muted">
              Caused by {cause.type}: {cause.message}
            </div>
          ))}
        </div>
      )}
      {node.traceback.length > 0 && (
        <ol className="traceback">
          {node.traceback.map((frame, index) => (
            <li key={index}>
              {frame.script ? (
                <button
                  className="frame-link"
                  onClick={() => onOpenLine?.(frame.line)}
                  title="Show this line in the editor"
                >
                  line {frame.line}
                </button>
              ) : (
                <span className="muted">
                  {frame.file}:{frame.line}
                </span>
              )}{" "}
              <span className="frame-function">{frame.function}</span>
              {frame.text && <code>{frame.text}</code>}
            </li>
          ))}
        </ol>
      )}
    </div>
  );
}

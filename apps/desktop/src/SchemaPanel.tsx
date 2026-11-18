import { useState } from "react";
import type { ConnectionSchema, SchemaTable } from "./schema";

const PAGE = 100;
function Table({ table }: { table: SchemaTable }) {
  const [open, setOpen] = useState(false);
  const [limit, setLimit] = useState(PAGE);
  return <div className="schema-table">
    <button aria-expanded={open} onClick={() => setOpen(!open)}><span className="tree-caret">{open ? "▾" : "▸"}</span>{table.schema}.{table.name} <span className="schema-count">{table.columns.length}</span></button>
    {open && <ul>{table.columns.slice(0, limit).map((column) => <li key={column.name}>
      <span>{column.name}</span>{column.primaryKey && <strong> PK</strong>} <code>{column.dataType || "untyped"}</code>
      {column.references.map((key, index) => <div key={index}>→ {key.schema}.{key.table}.{key.column}</div>)}
    </li>)}{limit < table.columns.length && <li><button onClick={() => setLimit(limit + PAGE)}>More columns</button></li>}</ul>}
  </div>;
}
export function SchemaPanel({ name, schema, loading, error, onRefresh }: {
  name?: string; schema?: ConnectionSchema; loading: boolean; error?: string; onRefresh: () => void;
}) {
  const [filter, setFilter] = useState("");
  const [limit, setLimit] = useState(PAGE);
  const tables = schema?.tables.filter((table) => `${table.schema}.${table.name}`.toLowerCase().includes(filter.toLowerCase())) ?? [];
  return <section className="sidebar-section schema-panel" aria-label="Connection schema">
    <h2><span>Schema{name && <span className="schema-name"> · {name}</span>}</span><button className="section-action" disabled={!name || loading} onClick={onRefresh}>Refresh schema</button></h2>
    {!name ? <p>Attach a connection in the script header to explore its schema.</p> : <>
      {loading && <p role="status">Loading schema…</p>}
      {error && <p role="alert">{error}{schema && " Showing the last loaded schema."}</p>}
      <input aria-label="Filter tables" placeholder="Filter tables" value={filter} onChange={(event) => { setFilter(event.target.value); setLimit(PAGE); }} />
      {schema && <p className="tree-status">{tables.length} tables and views</p>}
      <div className="schema-tables">{tables.slice(0, limit).map((table) => <Table key={JSON.stringify([table.schema, table.name])} table={table} />)}
        {tables.length > limit && <button onClick={() => setLimit(limit + PAGE)}>More tables</button>}
      </div>
    </>}
  </section>;
}

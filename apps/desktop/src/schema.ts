import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export type SchemaColumn = { name: string; dataType: string; primaryKey?: boolean; references: { schema: string; table: string; column: string }[] };
export type SchemaTable = { schema: string; name: string; columns: SchemaColumn[] };
export type ConnectionSchema = { kind: "sqlite" | "postgres" | "sqlserver"; tables: SchemaTable[] };

/** Retain a loaded snapshot offline; ignore responses for an old attachment. */
export function useConnectionSchema(name: string | undefined, revision: number) {
  const snapshots = useRef(new Map<string, ConnectionSchema>());
  const [result, setResult] = useState<{ name?: string; schema?: ConnectionSchema; error?: string; loading: boolean }>({ loading: false });
  useEffect(() => {
    let current = true;
    if (!name) { setResult({ loading: false }); return; }
    setResult({ name, schema: snapshots.current.get(name), loading: true });
    void invoke<ConnectionSchema>("connection_schema", { name }).then(
      (schema) => { if (current) { snapshots.current.set(name, schema); setResult({ name, schema, loading: false }); } },
      (error) => { if (current) setResult((old) => ({ ...old, error: String(error), loading: false })); },
    );
    return () => { current = false; };
  }, [name, revision]);
  return result.name === name ? result : { schema: name ? snapshots.current.get(name) : undefined, loading: !!name };
}

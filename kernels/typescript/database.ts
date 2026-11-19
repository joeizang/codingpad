import { SQL } from "bun";
import { Database, type SQLQueryBindings } from "bun:sqlite";
import { pathToFileURL } from "node:url";

type Parameter = string | number | bigint | boolean | Date | Uint8Array | null;
type Row = Record<string, unknown>;
type Backend = {
  query(sql: string, parameters: readonly Parameter[]): Row[] | Promise<Row[]>;
  close(): void | Promise<void>;
};

export type Connection = {
  settings: {
    kind: "sqlite" | "postgres" | "sqlserver";
    path: string | null;
    host: string | null;
    port: number | null;
    database: string | null;
    user: string | null;
    tls: "prefer" | "require" | "disable";
    trustServerCertificate: boolean;
  };
  password: string | null;
};

export function databaseSession(connection: Connection | null, sqlServerDriver: string | null) {
  let backend: Promise<Backend> | undefined;
  let closed = false;
  return {
    db: {
      async query(sql: string, parameters: readonly Parameter[] = []): Promise<Row[]> {
        if (closed) throw new Error("This database client belongs to a finished script run.");
        if (!connection) {
          throw new Error("No connection is attached. Add a codingpad: connection=<name> header.");
        }
        const client = await (backend ??= open(connection, sqlServerDriver));
        if (closed) throw new Error("This database client belongs to a finished script run.");
        return client.query(sql, parameters);
      },
    },
    async close() {
      closed = true;
      // A failed open already surfaced in the script's query.
      const client = await backend?.catch(() => undefined);
      await client?.close();
    },
  };
}

async function open({ settings, password }: Connection, sqlServerDriver: string | null): Promise<Backend> {
  if (settings.kind === "sqlite") {
    if (!settings.path?.trim()) throw new Error("This SQLite connection has no database file.");
    const client = new Database(settings.path.trim(), { strict: true });
    return {
      query: (sql, parameters) => client.query<Row, SQLQueryBindings[]>(sql).all(
        ...parameters.map((value) => value instanceof Date ? value.toISOString() : value),
      ),
      close: () => client.close(),
    };
  }

  const hostname = settings.host?.trim() || "localhost";
  const username = settings.user ?? "";
  const database = settings.database ?? "";
  if (settings.kind === "postgres") {
    const port = settings.port ?? 5432;
    const connect = async (ssl: "disable" | "require" | "verify-full") => {
      const client = new SQL("postgres://localhost", {
        hostname, port, username, database,
        path: hostname.startsWith("/") ? `${hostname}/.s.PGSQL.${port}` : undefined,
        password: password ?? "",
        ssl,
        tls: ssl === "verify-full" ? { rejectUnauthorized: true } : undefined,
        max: 1,
        connectionTimeout: 10,
      });
      try {
        // On Bun 1.4.2, connect() can emit a TLS error outside its promise.
        await client.unsafe("SELECT 1");
      } catch (error) {
        await client.close({ timeout: 0 });
        throw error;
      }
      return client;
    };
    const ssl = settings.tls === "disable" ? "disable"
      : settings.trustServerCertificate ? "require" : "verify-full";
    const client = await connect(ssl).catch((error) => {
      // Fall back only when the server explicitly says it has no TLS support.
      if (settings.tls === "prefer" && error.code === "ERR_POSTGRES_TLS_NOT_AVAILABLE") {
        return connect("disable");
      }
      throw error;
    });
    return {
      query: (sql, parameters) => client.unsafe(sql, [...parameters]),
      close: () => client.close({ timeout: 0 }),
    };
  }

  if (!sqlServerDriver) throw new Error("The TypeScript SQL Server driver is unavailable.");
  const { default: mssql } = await import(pathToFileURL(sqlServerDriver).href);
  const pool = await new mssql.ConnectionPool({
    server: hostname,
    port: settings.port ?? 1433,
    user: username,
    password: password ?? "",
    database,
    connectionTimeout: 10_000,
    options: {
      encrypt: settings.tls !== "disable",
      trustServerCertificate: settings.trustServerCertificate,
    },
  }).connect();
  const requests = new Set<{ cancel(): void }>();
  return {
    async query(sql, parameters) {
      const request = pool.request();
      parameters.forEach((value, index) => {
        request.input(`p${index + 1}`, value instanceof Uint8Array ? Buffer.from(value) : value);
      });
      requests.add(request);
      try {
        const result = await request.query(sql);
        return result.recordset ?? [];
      } finally {
        requests.delete(request);
      }
    },
    close: () => {
      for (const request of requests) request.cancel();
      return pool.close();
    },
  };
}

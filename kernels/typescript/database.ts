import { SQL } from "bun";
import { createRequire } from "node:module";
import { join } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { pathToFileURL } from "node:url";
import type { DatabaseClient, DatabaseParameter, DatabaseRows } from "codingpad";

type Backend = {
  query(sql: string, parameters: readonly DatabaseParameter[]): DatabaseRows | Promise<DatabaseRows>;
  execute(sql: string, parameters: readonly DatabaseParameter[]): number | Promise<number>;
  close(): void | Promise<void>;
};

const MAX_DATABASE_ROWS = 10_000;
const FINISHED_RUN = "This database client belongs to a finished script run.";
const queryResults = new WeakSet<object>();

export function isDatabaseRows(value: object): value is DatabaseRows {
  return queryResults.has(value);
}

function databaseRows(names: readonly string[], values: readonly (readonly unknown[])[]): DatabaseRows {
  const used = new Set<string>();
  const columns = names.map((name) => {
    let column = name;
    for (let suffix = 2; used.has(column); suffix++) column = `${name}_${suffix}`;
    used.add(column);
    return column;
  });
  const rows = values.slice(0, MAX_DATABASE_ROWS).map((row) =>
    Object.fromEntries(columns.map((column, index) => [column, row[index]])));
  const result = Object.assign(rows, { columns, truncated: values.length > MAX_DATABASE_ROWS });
  queryResults.add(result);
  return result;
}

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
  async function client() {
    if (closed) throw new Error(FINISHED_RUN);
    if (!connection) {
      throw new Error("No connection is attached. Add a codingpad: connection=<name> header.");
    }
    const current = await (backend ??= open(connection, sqlServerDriver));
    if (closed) throw new Error(FINISHED_RUN);
    return current;
  }
  const db: DatabaseClient = {
    async query(sql, parameters = []) {
      return (await client()).query(sql, parameters);
    },
    async execute(sql, parameters = []) {
      return (await client()).execute(sql, parameters);
    },
  };
  return {
    db,
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
    const client = new DatabaseSync(settings.path.trim(), {
      enableForeignKeyConstraints: false,
      enableDoubleQuotedStringLiterals: true,
    });
    const bind = (parameters: readonly DatabaseParameter[]) =>
      parameters.map((value) => {
        if (value instanceof Date) return value.toISOString();
        return typeof value === "boolean" ? Number(value) : value;
      });
    return {
      query(sql, parameters) {
        const statement = client.prepare(sql);
        statement.setReturnArrays(true);
        const rows: unknown[][] = [];
        for (const row of statement.iterate(...bind(parameters))) {
          if (!Array.isArray(row)) throw new Error("Bun SQLite did not return positional rows.");
          rows.push(row);
          if (rows.length > MAX_DATABASE_ROWS) break;
        }
        return databaseRows(statement.columns().map((column) => column.name), rows);
      },
      execute: (sql, parameters) => Number(client.prepare(sql).run(...bind(parameters)).changes),
      close: () => client.close(),
    };
  }

  const hostname = settings.host?.trim() || "localhost";
  const username = settings.user ?? "";
  const database = settings.database ?? "";
  if (settings.kind === "postgres") {
    if (!Bun.semver.satisfies(Bun.version, ">=1.4.4")) {
      throw new Error("TypeScript PostgreSQL connections require Bun 1.4.4 or later. Run bun upgrade and retry.");
    }
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
      async query(sql, parameters) {
        let declared = false;
        const rows = await client.begin(async (transaction) => {
          await transaction.unsafe(`DECLARE codingpad_query NO SCROLL CURSOR FOR ${sql}`, [...parameters]);
          declared = true;
          return await transaction.unsafe<unknown[][]>(
            `FETCH FORWARD ${MAX_DATABASE_ROWS + 1} FROM codingpad_query`,
          ).values();
        }).catch((error) => {
          // PostgreSQL cursors accept row selections, but not commands such as
          // SHOW or INSERT ... RETURNING. The failed declaration ran no command.
          if (!declared && (error.errno === "42601" || error.errno === "0A000")) {
            return client.unsafe<unknown[][]>(sql, [...parameters]).values();
          }
          throw error;
        });
        return databaseRows(rows.columns?.map((column) => column.name) ?? [], rows);
      },
      async execute(sql, parameters) {
        const result = await client.unsafe(sql, [...parameters]);
        return result.affectedRows;
      },
      close: () => client.close({ timeout: 0 }),
    };
  }

  if (!sqlServerDriver) throw new Error("The TypeScript SQL Server driver is unavailable.");
  const requireDriver = createRequire(pathToFileURL(join(sqlServerDriver, "package.json")));
  const mssql = requireDriver("mssql");
  const pool = await new mssql.ConnectionPool({
    server: hostname,
    port: settings.port ?? 1433,
    user: username,
    password: password ?? "",
    database,
    connectionTimeout: 10_000,
    requestTimeout: 0,
    options: {
      encrypt: settings.tls !== "disable",
      trustServerCertificate: settings.trustServerCertificate,
    },
  }).connect();
  const requests = new Set<{ cancel(): void }>();
  async function run(sql: string, parameters: readonly DatabaseParameter[], collectRows: boolean) {
    const request = pool.request();
    request.arrayRowMode = true;
    request.stream = true;
    parameters.forEach((value, index) => {
      request.input(`p${index + 1}`, value instanceof Uint8Array ? Buffer.from(value) : value);
    });
    let columns: string[] | undefined;
    let firstRecordset = false;
    const rows: unknown[][] = [];
    let requestError: Error | undefined;
    request.on("error", (error: Error) => { requestError ??= error; });
    if (collectRows) {
      request.on("recordset", (metadata: { name: string }[]) => {
        firstRecordset = columns === undefined;
        if (firstRecordset) columns = metadata.map((column) => column.name);
      });
      request.on("row", (row: unknown[]) => {
        if (firstRecordset && rows.length <= MAX_DATABASE_ROWS) rows.push(row);
      });
    }
    requests.add(request);
    try {
      // Drain the full request so batches and writes finish, but retain only
      // enough of the first result set to detect truncation.
      const result: { rowsAffected?: number[] } = await request.query(sql);
      if (requestError) throw requestError;
      return { columns: columns ?? [], rows, rowsAffected: result.rowsAffected ?? [] };
    } finally {
      requests.delete(request);
    }
  }
  return {
    async query(sql, parameters) {
      const { rows, columns } = await run(sql, parameters, true);
      return databaseRows(columns, rows);
    },
    async execute(sql, parameters) {
      const { rowsAffected } = await run(sql, parameters, false);
      return rowsAffected.reduce((total, count) => total + count, 0);
    },
    close: () => {
      for (const request of requests) request.cancel();
      return pool.close();
    },
  };
}

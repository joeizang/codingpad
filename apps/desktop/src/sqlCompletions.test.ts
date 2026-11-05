import { describe, expect, it } from "vitest";
import { createSqlCompletions } from "./sqlCompletions";
import type { ConnectionSchema } from "./schema";
const schema: ConnectionSchema = {
  kind: "postgres",
  tables: [
    {
      schema: "public",
      name: "users",
      columns: [
        { name: "user_id", dataType: "int", references: [] },
        { name: "email", dataType: "text", references: [] },
      ],
    },
    {
      schema: "public",
      name: "orders",
      columns: [{ name: "order_id", dataType: "int", references: [] }],
    },
    {
      schema: "archive",
      name: "users",
      columns: [{ name: "archived_at", dataType: "date", references: [] }],
    },
    {
      schema: "public",
      name: "odd table",
      columns: [{ name: 'a"b', dataType: "text", references: [] }],
    },
  ],
};
function complete(query: string, input = schema) {
  const offset = query.indexOf("|");
  return createSqlCompletions(input)(query.replace("|", ""), offset);
}
const columns = (query: string) =>
  complete(query)
    .suggestions.filter((s) => s.kind === "column")
    .map((s) => s.label);
describe("SQL schema completions", () => {
  it("offers real tables and bundled keywords without a kernel", () => {
    expect(
      complete("SELECT * FROM us|").suggestions.map((s) => s.label),
    ).toEqual(["public.users", "archive.users"]);
    expect(complete("SEL|").suggestions.map((s) => s.label)).toContain(
      "SELECT",
    );
    expect(createSqlCompletions(undefined)("SEL", 3).suggestions[0].label).toBe(
      "SELECT",
    );
    expect(columns("SELECT | FROM public.users")).toEqual(["user_id", "email"]);
    expect(columns("SELECT | FROM public.orders")).toEqual(["order_id"]);
  });
  it("offers dialect keywords from the attached connection", () => {
    expect(
      complete("LIM|", { ...schema, kind: "sqlserver" }).suggestions,
    ).toEqual([]);
    expect(
      complete("TOP|", { ...schema, kind: "sqlserver" }).suggestions[0].label,
    ).toBe("TOP");
    expect(complete("TOP|", { ...schema, kind: "sqlite" }).suggestions).toEqual(
      [],
    );
    expect(complete("LIM|", schema).suggestions[0].label).toBe("LIMIT");
  });
  it("resolves aliases, joins, schema qualifiers, and comma-separated tables", () => {
    expect(
      columns(
        "SELECT u.| FROM public.users AS u JOIN public.orders o ON u.user_id=o.order_id",
      ),
    ).toEqual(["user_id", "email"]);
    expect(columns("SELECT o.| FROM public.users u, public.orders o")).toEqual([
      "order_id",
    ]);
    expect(
      complete("SELECT * FROM public.|").suggestions.map((s) => s.label),
    ).toEqual(["users", "orders", "odd table"]);
    expect(columns("SELECT public.users.| FROM public.users")).toEqual([
      "user_id",
      "email",
    ]);
    expect(columns("SELECT missing.| FROM public.users")).toEqual([]);
  });
  it("uses the current statement and handles nested scope with alias shadowing", () => {
    expect(
      columns("SELECT * FROM public.orders; SELECT | FROM public.users;"),
    ).toEqual(["user_id", "email"]);
    expect(
      columns(
        "SELECT | FROM public.users WHERE EXISTS (SELECT * FROM public.orders)",
      ),
    ).toEqual(["user_id", "email"]);
    expect(
      columns("SELECT (SELECT u.| FROM public.orders u) FROM public.users u"),
    ).toEqual(["order_id"]);
    expect(columns("SELECT (SELECT u.|) FROM public.users u")).toEqual([
      "user_id",
      "email",
    ]);
    expect(columns("SELECT count(|) FROM public.users")).toEqual([
      "user_id",
      "email",
    ]);
  });
  it("ignores comments and strings, including PostgreSQL dollar quotes", () => {
    for (const query of [
      "SELECT 'FROM public.orders; |' FROM public.users",
      "-- FROM public.orders |",
      "SELECT $$ FROM public.orders; | $$",
      "/* outer /* inner */ | */ SELECT * FROM public.users",
    ])
      expect(complete(query).suggestions).toEqual([]);
    expect(
      columns(
        "SELECT |, 'FROM public.orders' FROM public.users -- JOIN public.orders",
      ),
    ).toEqual(["user_id", "email"]);
    expect(
      columns("SELECT | FROM public.users /* ; FROM public.orders */"),
    ).toEqual(["user_id", "email"]);
  });
  it("does not invent columns for CTE or derived-table projections", () => {
    expect(
      columns(
        "WITH users AS (SELECT order_id FROM public.orders) SELECT users.| FROM users",
      ),
    ).toEqual([]);
    expect(
      columns(
        "SELECT derived.| FROM (SELECT user_id FROM public.users) derived",
      ),
    ).toEqual([]);
  });
  it("quotes identifiers for each dialect and replaces a quoted token", () => {
    expect(columns('SELECT x.| FROM "public"."odd table" x')).toEqual(['a"b']);
    expect(
      complete('SELECT x.| FROM "public"."odd table" x').suggestions[0]
        .insertText,
    ).toBe('"a""b"');
    const mssql: ConnectionSchema = {
      kind: "sqlserver",
      tables: [
        {
          schema: "dbo",
          name: "odd]table",
          columns: [{ name: "a]b", dataType: "int", references: [] }],
        },
      ],
    };
    expect(
      complete("SELECT t.| FROM [dbo].[odd]]table] t", mssql).suggestions[0]
        .insertText,
    ).toBe("[a]]b]");
    const result = complete('SELECT "em|ail" FROM public.users');
    expect(
      result.suggestions.find((s) => s.label === "email")?.insertText,
    ).toBe('"email"');
    expect(result.end - result.start).toBe(7);
  });
  it("scopes UPDATE, INSERT INTO, and DELETE FROM", () => {
    expect(columns("UPDATE public.users SET | WHERE user_id=1")).toEqual([
      "user_id",
      "email",
    ]);
    expect(columns("INSERT INTO public.users (|) VALUES (1,'a')")).toEqual([
      "user_id",
      "email",
    ]);
    expect(columns("DELETE FROM public.orders WHERE |")).toEqual(["order_id"]);
  });
  it("completes a large cached schema within D38 and caps returned suggestions", () => {
    const large: ConnectionSchema = {
      kind: "sqlite",
      tables: Array.from({ length: 10000 }, (_, i) => ({
        schema: "main",
        name: `table_${i}`,
        columns: Array.from({ length: 40 }, (_, j) => ({
          name: `column_${j}`,
          dataType: "TEXT",
          references: [],
        })),
      })),
    };
    const cached = createSqlCompletions(large);
    const start = performance.now();
    let slowest = 0;
    for (let i = 0; i < 20; i++) {
      const requestStart = performance.now();
      const result = cached("SELECT t. FROM main.table_9999 t", 9);
      slowest = Math.max(slowest, performance.now() - requestStart);
      expect(result.suggestions).toHaveLength(40);
    }
    const perRequest = (performance.now() - start) / 20;
    console.info(
      `SQL completion: ${perRequest.toFixed(2)} ms/request, max ${slowest.toFixed(2)} ms (10,000 tables / 400,000 columns)`,
    );
    expect(slowest).toBeLessThan(300);
    expect(
      cached("SELECT  FROM main.table_9999", 7).suggestions.some(
        (s) => s.label === "column_0",
      ),
    ).toBe(true);
    const capped = cached("SELECT * FROM ", 14);
    expect(capped.suggestions).toHaveLength(200);
    expect(capped.incomplete).toBe(true);
  });
});

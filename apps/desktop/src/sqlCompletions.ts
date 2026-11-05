import type { ConnectionSchema, SchemaTable } from "./schema";

const KEYWORDS =
  "SELECT FROM WHERE JOIN LEFT RIGHT INNER OUTER FULL CROSS ON AS AND OR NOT NULL IS IN EXISTS LIKE BETWEEN DISTINCT GROUP BY ORDER HAVING LIMIT OFFSET UNION ALL INSERT INTO VALUES UPDATE SET DELETE CREATE TABLE ALTER DROP WITH CASE WHEN THEN ELSE END ASC DESC COUNT SUM AVG MIN MAX RETURNING TOP FETCH".split(
    " ",
  );
const RESERVED = new Set(KEYWORDS);
export type SqlSuggestion = {
  label: string;
  insertText: string;
  kind: "table" | "column" | "keyword";
  detail?: string;
};
type OpaqueRange = { start: number; end: number; endInclusive: boolean };
type Token = {
  value: string;
  start: number;
  end: number;
  identifier: boolean;
  quoted: boolean;
  scope: number;
};

/** A small lexical scanner, not a SQL parser. Strings/comments cannot introduce
 * tables, statements, or qualifiers. Offsets remain Monaco UTF-16 offsets. */
function tokenize(source: string): {
  tokens: Token[];
  scopes: number[];
  ranges: [number, number][];
  opaque: OpaqueRange[];
} {
  const tokens: Token[] = [],
    scopes = [-1],
    stack = [0],
    opaque: OpaqueRange[] = [];
  const ranges: [number, number][] = [[0, source.length]];
  let i = 0;
  while (i < source.length) {
    const start = i,
      char = source[i];
    if (/\s/.test(char)) {
      i++;
      continue;
    }
    if (source.startsWith("--", i) || source.startsWith("/*", i)) {
      const line = source.startsWith("--", i);
      i += 2;
      let depth = 1;
      while (i < source.length) {
        if (line && source[i] === "\n") break;
        if (!line && source.startsWith("/*", i)) {
          depth++;
          i += 2;
        } else if (!line && source.startsWith("*/", i)) {
          i += 2;
          if (--depth === 0) break;
        } else i++;
      }
      opaque.push({ start, end: i, endInclusive: line || depth > 0 });
      continue;
    }
    const dollar =
      char === "$"
        ? source.slice(i).match(/^\$(?:[A-Za-z_][\w]*)?\$/)?.[0]
        : undefined;
    if (char === "'" || dollar) {
      let closed = false;
      if (dollar) {
        const end = source.indexOf(dollar, i + dollar.length);
        closed = end >= 0;
        i = end < 0 ? source.length : end + dollar.length;
      } else {
        const escaped = start > 0 && /[eE]/.test(source[start - 1]);
        i++;
        while (i < source.length) {
          if (escaped && source[i] === "\\") {
            i = Math.min(i + 2, source.length);
            continue;
          }
          if (source[i++] === "'") {
            if (source[i] !== "'") {
              closed = true;
              break;
            }
            i++;
          }
        }
      }
      opaque.push({ start, end: i, endInclusive: !closed });
      continue;
    }
    let value = "",
      identifier = false,
      quoted = false;
    if (char === '"' || char === "[" || char === "`") {
      identifier = quoted = true;
      const end = char === "[" ? "]" : char;
      i++;
      while (i < source.length) {
        const next = source[i++];
        if (next === end) {
          if (source[i] !== end) break;
          i++;
        }
        value += next;
      }
    } else if (/[A-Za-z_\u0080-\uffff]/.test(char)) {
      identifier = true;
      while (i < source.length && /[\w$\u0080-\uffff]/.test(source[i]))
        value += source[i++];
    } else {
      value = char;
      i++;
    }
    const scope = stack[stack.length - 1];
    tokens.push({ value, start, end: i, identifier, quoted, scope });
    if (value === "(") {
      scopes.push(scope);
      ranges.push([i, source.length]);
      stack.push(scopes.length - 1);
    } else if (value === ")" && stack.length > 1) {
      ranges[stack.pop()!][1] = start;
    }
  }
  return { tokens, scopes, ranges, opaque };
}
function lastIndex<T>(items: T[], predicate: (item: T) => boolean): number {
  for (let i = items.length - 1; i >= 0; i--) if (predicate(items[i])) return i;
  return -1;
}
const key = (value: string) => value.toLowerCase();
const keyword = (token: Token | undefined, word: string) =>
  !!token && !token.quoted && token.value.toUpperCase() === word;
const isName = (token: Token | undefined) =>
  !!token?.identifier &&
  (token.quoted || !RESERVED.has(token.value.toUpperCase()));

export function createSqlCompletions(schema: ConnectionSchema | undefined) {
  const keywords = KEYWORDS.filter((word) => {
    if (!schema) return true;
    if (schema.kind === "sqlserver")
      return word !== "LIMIT" && word !== "RETURNING";
    return word !== "TOP" && (schema.kind !== "sqlite" || word !== "FETCH");
  });
  const quote = (name: string) =>
    schema?.kind === "sqlserver"
      ? `[${name.replaceAll("]", "]]")}]`
      : `"${name.replaceAll('"', '""')}"`;
  const tableSuggestions = (schema?.tables ?? []).map((table) => ({
    table,
    suggestion: {
      label: `${table.schema}.${table.name}`,
      insertText: `${quote(table.schema)}.${quote(table.name)}`,
      kind: "table" as const,
    },
  }));
  const byName = new Map<string, SchemaTable[]>();
  for (const table of schema?.tables ?? []) {
    for (const name of [table.name, `${table.schema}.${table.name}`]) {
      const normalized = key(name);
      const list = byName.get(normalized) ?? [];
      list.push(table);
      byName.set(normalized, list);
    }
  }
  return (
    source: string,
    offset: number,
  ): {
    suggestions: SqlSuggestion[];
    incomplete: boolean;
    start: number;
    end: number;
  } => {
    const lexed = tokenize(source);
    if (lexed.opaque.some(({ start, end, endInclusive }) =>
      offset > start && (offset < end || (endInclusive && offset === end)),
    ))
      return { suggestions: [], incomplete: false, start: offset, end: offset };
    // Semicolons inside comments and literals were removed by the scanner.
    const begin = lastIndex(
      lexed.tokens,
      (token) => token.value === ";" && token.end <= offset,
    );
    const rest = lexed.tokens.slice(begin + 1);
    const finish = rest.findIndex(
      (token) => token.value === ";" && token.start >= offset,
    );
    const tokens = finish < 0 ? rest : rest.slice(0, finish);
    const active = tokens.find(
      (token) =>
        token.identifier && token.start < offset && token.end >= offset,
    );
    const start = active?.start ?? offset,
      end = active?.end ?? offset;
    const prefix = active
      ? key(
          source
            .slice(active.start + (active.quoted ? 1 : 0), offset)
            .replace(/["\]`]$/, ""),
        )
      : "";
    const before = tokens.filter((token) => token.end <= start);
    const previous = before.at(-1);
    let scope = lastIndex(
      lexed.ranges,
      ([start, end]) => offset >= start && offset <= end,
    );
    const ancestors = new Set<number>();
    while (scope >= 0) {
      ancestors.add(scope);
      scope = lexed.scopes[scope] ?? -1;
    }
    // Only tables in this block and its ancestors are visible. Descendant
    // subqueries cannot leak their columns into the enclosing statement.
    const references = new Map<string, SchemaTable[]>();
    const referenceScopes = new Map<string, number>();
    const setReference = (
      name: string,
      tables: SchemaTable[],
      scope: number,
    ) => {
      name = key(name);
      if ((referenceScopes.get(name) ?? -1) <= scope) {
        references.set(name, tables);
        referenceScopes.set(name, scope);
      }
    };
    const cteNames = new Set<string>();
    for (let i = 0; i < tokens.length - 2; i++) {
      if (
        isName(tokens[i]) &&
        keyword(tokens[i + 1], "AS") &&
        tokens[i + 2].value === "("
      )
        cteNames.add(key(tokens[i].value));
    }
    const visible = new Set<SchemaTable>();
    const fromScopes = new Set<number>();
    let tableContext = false;
    for (let i = 0; i < tokens.length; i++) {
      const token = tokens[i];
      if (!ancestors.has(token.scope)) continue;
      if (keyword(token, "FROM")) fromScopes.add(token.scope);
      if (
        [
          "WHERE",
          "GROUP",
          "ORDER",
          "HAVING",
          "SET",
          "VALUES",
          "ON",
          "UNION",
        ].some((word) => keyword(token, word))
      )
        fromScopes.delete(token.scope);
      const introduces =
        ["FROM", "JOIN", "UPDATE", "INTO"].some((word) =>
          keyword(token, word),
        ) ||
        (token.value === "," && fromScopes.has(token.scope));
      if (!introduces) continue;
      if (token === previous) tableContext = true;
      let j = i + 1;
      if (!isName(tokens[j])) continue;
      const parts = [tokens[j].value];
      let tableEnd = tokens[j].end;
      while (tokens[j + 1]?.value === "." && isName(tokens[j + 2])) {
        j += 2;
        parts.push(tokens[j].value);
        tableEnd = tokens[j].end;
      }
      if (offset >= tokens[i + 1].start && offset <= tableEnd)
        tableContext = true;
      const tables =
        parts.length === 1 && cteNames.has(key(parts[0]))
          ? []
          : (byName.get(key(parts.join("."))) ?? []);
      for (const table of tables) visible.add(table);
      setReference(parts.join("."), tables, token.scope);
      setReference(parts.at(-1)!, tables, token.scope);
      if (keyword(tokens[j + 1], "AS")) j++;
      if (isName(tokens[j + 1]) && tokens[j + 1].scope === token.scope)
        setReference(tokens[j + 1].value, tables, token.scope);
    }
    let candidates: SqlSuggestion[] = [];
    if (previous?.value === ".") {
      const parts: string[] = [];
      for (let i = before.length - 2; i >= 0 && before[i].identifier; i -= 2) {
        parts.unshift(before[i].value);
        if (before[i - 1]?.value !== ".") break;
      }
      const qualifier = key(parts.join("."));
      const tables = references.get(qualifier);
      if (tables?.length && !tableContext) {
        candidates = tables.flatMap((table) =>
          table.columns.map((column) => ({
            label: column.name,
            insertText: quote(column.name),
            kind: "column" as const,
            detail: `${table.schema}.${table.name} · ${column.dataType}`,
          })),
        );
      } else {
        candidates = tableSuggestions
          .filter(({ table }) => key(table.schema) === qualifier)
          .map(({ table }) => ({
            label: table.name,
            insertText: quote(table.name),
            kind: "table" as const,
          }));
      }
    } else {
      if (!tableContext) {
        candidates.push(
          ...Array.from(visible).flatMap((table) =>
            table.columns.map((column) => ({
              label: column.name,
              insertText: quote(column.name),
              kind: "column" as const,
              detail: `${table.schema}.${table.name} · ${column.dataType}`,
            })),
          ),
        );
        candidates.push(
          ...keywords.map((label) => ({
            label,
            insertText: label,
            kind: "keyword" as const,
          })),
        );
      }
      candidates = candidates.concat(
        tableSuggestions.map(({ suggestion }) => suggestion),
      );
    }
    const seen = new Set<string>();
    const suggestions: SqlSuggestion[] = [];
    for (const candidate of candidates) {
      const match = key(candidate.label);
      if (
        !match.startsWith(prefix) &&
        !(
          candidate.kind === "table" &&
          match.split(".").at(-1)!.startsWith(prefix)
        )
      )
        continue;
      const identity = `${candidate.kind}:${candidate.insertText}:${candidate.detail ?? ""}`;
      if (seen.has(identity)) continue;
      seen.add(identity);
      if (suggestions.length === 200)
        return { suggestions, incomplete: true, start, end };
      suggestions.push(candidate);
    }
    return { suggestions, incomplete: false, start, end };
  };
}

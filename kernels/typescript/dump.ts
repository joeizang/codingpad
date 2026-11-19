import { types } from "node:util";
import { Html, Image, Live } from "./runtime.ts";
import { inspectValue, MAX_TEXT } from "./errors.ts";

const PAGE_SIZE = 100;
const MAX_DEPTH = 6;
const MAX_NODES = 1_000;
const MAX_PREVIEW_TEXT = 6 * 1024 * 1024;

export type PageRequest = { handle: string; cursor: string | null; limit: number | null };
export const missingHandle = "That preview is no longer available. Run the script again.";

type Node = { id: string; kind: string; [key: string]: unknown };
type Budget = { nodes: number; text: number };
type Page = (offset: number, size: number) => Node | Promise<Node>;
type ExportPage = (offset: number, size: number) => Promise<object>;
type Iteration = { iterator: Iterator<unknown> | AsyncIterator<unknown>; async: boolean };
const accessor = Symbol("accessor");

function budget(): Budget {
  return { nodes: MAX_NODES, text: MAX_PREVIEW_TEXT };
}

function property(value: object, key: PropertyKey): unknown {
  const own = Object.getOwnPropertyDescriptor(value, key);
  return own && ("value" in own ? own.value : accessor);
}

function useTextPreview(value: object) {
  return types.isProxy(value) || types.isDate(value) || types.isMap(value) || types.isSet(value)
    || types.isPromise(value) || types.isRegExp(value) || types.isArrayBufferView(value)
    || types.isAnyArrayBuffer(value);
}

function record(value: unknown): value is object {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    && !useTextPreview(value) && !(value instanceof Html || value instanceof Image || value instanceof Live)
    && !types.isNativeError(value) && !iterable(value);
}

// Read data methods from the prototype chain without invoking an accessor.
function iteratorMethod(value: object, key: symbol): Function | undefined {
  for (let current: object | null = value; current && !types.isProxy(current); current = Object.getPrototypeOf(current)) {
    const descriptor = Object.getOwnPropertyDescriptor(current, key);
    if (descriptor) return typeof descriptor.value === "function" ? descriptor.value : undefined;
  }
}

function iterable(value: object) {
  return iteratorMethod(value, Symbol.asyncIterator) ?? iteratorMethod(value, Symbol.iterator);
}

function isSequence(value: unknown): value is object {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    && !useTextPreview(value) && Boolean(iterable(value));
}

function columnsFor(values: Iterable<unknown>, limit = 32) {
  const names = new Set<string>();
  for (const value of values) {
    if (!record(value)) continue;
    for (const key of Object.keys(value)) {
      if (names.size >= limit) return [...names];
      names.add(key);
    }
  }
  return [...names];
}

function exportCell(value: unknown) {
  if (value == null) return null;
  if (typeof value === "bigint") return { n: String(value) };
  if (typeof value === "string" || typeof value === "boolean") return value;
  if (typeof value === "number") return Number.isFinite(value) ? value : String(value);
  if (value === accessor) return "[Accessor]";
  return inspectValue(value);
}

export class Dumps {
  #nextId = 0;
  #handles = new Map<string, { page: Page; exportPage?: ExportPage }>();
  #pending = new Set<Promise<void>>();
  #unsubscribe: (() => void)[] = [];
  #running = true;
  #openIterators = new Set<Iterator<unknown> | AsyncIterator<unknown>>();

  constructor(
    private readonly runId: number,
    private readonly notify: (method: string, params: object) => void,
    private readonly describeError: (error: unknown) => object,
  ) {}

  #id() {
    return `${this.runId}:n${++this.#nextId}`;
  }

  error(error: unknown): Node {
    return { id: this.#id(), kind: "error", ...this.describeError(error) };
  }

  #keep(page: Page, exportPage?: ExportPage) {
    const handle = `${this.runId}:h${this.#handles.size + 1}`;
    this.#handles.set(handle, { page, exportPage });
    return handle;
  }

  dump(value: unknown) {
    if (!this.#running) return;
    if (isSequence(value)) {
      const pending = this.#sequence(value, []).then(
        (node) => this.notify("dump", { node }),
        (error) => this.notify("dump", { node: this.error(error) }),
      ).finally(() => this.#pending.delete(pending));
      this.#pending.add(pending);
      return;
    }
    this.notify("dump", { node: this.#preview(value, budget()) });
  }

  get canExpand() {
    return this.#handles.size > 0;
  }

  async finish() {
    while (this.#pending.size) await Promise.all(this.#pending);
    this.#running = false;
    for (const unsubscribe of this.#unsubscribe) unsubscribe();
    this.#unsubscribe.length = 0;
  }

  async dispose() {
    await Promise.allSettled([...this.#openIterators].map(async (iterator) => {
      await iterator.return?.();
    }));
  }

  async page({ handle, cursor, limit }: PageRequest, exporting: boolean) {
    const entry = this.#handles.get(handle);
    if (!entry) return { error: { code: -32001, message: missingHandle } };
    const offset = cursor == null || /^\d+$/.test(cursor) ? Number(cursor ?? 0) : NaN;
    const size = limit ?? PAGE_SIZE;
    if (!Number.isSafeInteger(offset) || !Number.isSafeInteger(size) || size < 1) {
      return { error: { code: -32602, message: "Invalid page cursor or limit." } };
    }
    try {
      if (exporting) {
        if (!entry.exportPage) throw new Error("Only table dumps can be exported.");
        return { result: await entry.exportPage(offset, Math.min(size, PAGE_SIZE)) };
      }
      return { result: { node: await entry.page(offset, Math.min(size, PAGE_SIZE)) } };
    } catch (error) {
      const node = this.error(error);
      return { error: { code: -32603, message: String(node.message) } };
    }
  }

  #lazy(id: string, value: unknown, ancestors: object[]): Node {
    const handle = this.#keep(() => isSequence(value)
      ? this.#sequence(value, ancestors)
      : this.#preview(value, budget(), ancestors));
    const preview = Array.isArray(value) ? "Array" : typeof value === "string" ? "String" : "Expand value";
    return { id, kind: "lazy", preview, handle };
  }

  #preview(value: unknown, remaining: Budget, ancestors: object[] = [], depth = 0, cell = false): Node {
    const id = this.#id();
    remaining.nodes--;
    const string = (text: string) => {
      const v = text.slice(0, cell ? 1_000 : MAX_TEXT);
      if (v.length * 6 > remaining.text) return this.#lazy(id, text, []);
      remaining.text -= v.length * 6;
      return { id, kind: "string", v, truncated: v.length < text.length, total: text.length };
    };
    if (value === null || (cell && value === undefined)) return { id, kind: "null" };
    if (typeof value === "boolean" || (typeof value === "number" && Number.isFinite(value))) {
      return { id, kind: "primitive", v: value };
    }
    if (value === accessor) return string("[Accessor]");
    if (typeof value === "string") return string(value);
    if (typeof value !== "object" || useTextPreview(value)) return string(inspectValue(value));
    if (ancestors.includes(value)) return string("[Circular]");
    if (cell || depth >= MAX_DEPTH || remaining.nodes < 1 || remaining.text < 1_200_000) {
      return this.#lazy(id, value, ancestors);
    }

    if (value instanceof Html) {
      remaining.text -= value.markup.length * 6;
      return { id, kind: "html", html: value.markup };
    }
    if (value instanceof Image) {
      if (value.src.length > remaining.text) return this.#lazy(id, value, ancestors);
      remaining.text -= value.src.length;
      return { id, kind: "image", src: value.src, mime: value.mime, name: value.name };
    }
    if (types.isNativeError(value)) return this.error(value);
    if (value instanceof Live) {
      const path = [...ancestors, value];
      const node = this.#preview(value.value, remaining, path, depth);
      if (!this.#running) return { id, kind: "live", node };
      const update = (current: unknown) => this.notify("dump_update", {
        nodeId: node.id, node: this.#preview(current, budget(), path),
      });
      value.listeners.add(update);
      this.#unsubscribe.push(() => value.listeners.delete(update));
      return { id, kind: "live", node };
    }
    if (!Array.isArray(value) && iterable(value)) return this.#lazy(id, value, ancestors);

    const path = [...ancestors, value];
    if (Array.isArray(value) && record(property(value, "0"))) {
      const columns = columnsFor(Array.from({ length: Math.min(PAGE_SIZE, value.length) },
        (_, index) => property(value, index)));
      if (columns.length) {
        return depth === 0 ? this.#arrayTable(id, value, columns, path) : this.#lazy(id, value, ancestors);
      }
    }
    return this.#collection(id, value, path, remaining, depth);
  }

  #collection(id: string, value: object, path: object[], remaining: Budget, depth: number): Node {
    const keys = Array.isArray(value) ? undefined : Object.keys(value);
    const proto = Object.getPrototypeOf(value);
    const constructor = proto && !types.isProxy(proto) ? property(proto, "constructor") : undefined;
    const name = typeof constructor === "function" && !types.isProxy(constructor)
      ? property(constructor, "name") : undefined;
    const type = typeof name === "string" && name ? name.slice(0, MAX_TEXT) : "Object";
    const total = Array.isArray(value) ? value.length : keys!.length;
    let handle: string | undefined;
    const read = (id: string, offset: number, size: number, remaining: Budget, depth: number): Node => {
      const items = [];
      const fields = [];
      for (let index = offset; index < Math.min(total, offset + size) && remaining.nodes > 0; index++) {
        const key = keys?.[index];
        if (key !== undefined) {
          const name = key.slice(0, MAX_TEXT);
          if (name.length * 6 > remaining.text) break;
          remaining.text -= name.length * 6;
          fields.push({ name, value: this.#preview(property(value, key), remaining, path, depth + 1) });
        } else {
          items.push(this.#preview(property(value, index), remaining, path, depth + 1));
        }
      }
      const end = offset + items.length + fields.length;
      const next = end < total ? String(end) : null;
      if (next && !handle) handle = this.#keep((offset, size) => read(this.#id(), offset, size, budget(), 0));
      return { id, ...(keys ? { kind: "object", fields } : { kind: "list", items }),
        type, total, next, truncated: false, handle };
    };
    return read(id, 0, PAGE_SIZE, remaining, depth);
  }

  #arrayTable(id: string, value: unknown[], columns: string[], path: object[]): Node {
    const total = value.length;
    const slice = (offset: number, size: number) =>
      Array.from({ length: Math.max(0, Math.min(size, total - offset)) }, (_, index) => property(value, index + offset));
    const cells = (row: unknown, names: string[]) => record(row)
      ? names.map((key) => property(row, key))
      : names.map(() => undefined);
    const page = (id: string, offset: number, size: number): Node => {
      const remaining = budget();
      const rows = slice(offset, size).map((row) =>
        cells(row, columns).map((cell) => this.#preview(cell, remaining, path, 0, true)));
      const end = offset + rows.length;
      return { id, kind: "table", type: "Array", columns: columns.map((name) => ({ name: name.slice(0, MAX_TEXT) })),
        rows, total, next: end < total ? String(end) : null, handle };
    };
    let exportColumns: string[] | undefined;
    const handle = this.#keep((offset, size) => page(this.#id(), offset, size), async (offset, size) => {
      exportColumns ??= columnsFor((function* () {
        for (let index = 0; index < total; index++) yield property(value, index);
      })(), Infinity);
      const names = exportColumns;
      const rows = slice(offset, size).map((row) => cells(row, names).map(exportCell));
      const end = offset + rows.length;
      return { columns: names, rows, total, next: end < total ? String(end) : null };
    });
    return page(id, 0, PAGE_SIZE);
  }

  async #sequence(value: object, ancestors: object[]): Promise<Node> {
    const asyncMethod = iteratorMethod(value, Symbol.asyncIterator);
    const method = asyncMethod ?? iteratorMethod(value, Symbol.iterator)!;
    const iteration: Iteration = { iterator: method.call(value), async: asyncMethod !== undefined };
    this.#openIterators.add(iteration.iterator);
    const path = [...ancestors, value];
    const values: unknown[] = [];
    let done = false;
    let failure: Node | undefined;

    const read = async (offset: number, size: number) => {
      if (offset > values.length + Number(failure !== undefined)) {
        throw new RangeError("Read the previous page before requesting more rows.");
      }
      while (!done && values.length < offset + size) {
        try {
          const next = iteration.iterator.next();
          const item = iteration.async ? await next : next as IteratorResult<unknown>;
          if (typeof item !== "object" || item === null) throw new TypeError("Invalid iterator result");
          done = Boolean(item.done);
          if (done) this.#openIterators.delete(iteration.iterator);
          else values.push(item.value);
        } catch (error) {
          done = true;
          failure = this.error(error);
        }
      }
      return values.slice(offset, offset + size);
    };

    const initial = await read(0, PAGE_SIZE);
    const names = record(initial[0]) ? columnsFor(initial) : [];
    const columns = names.length ? names : ["Value"];
    const cells = (row: unknown) => names.length
      ? (record(row) ? names.map((key) => property(row, key)) : names.map(() => undefined))
      : [row];
    const page = (offset: number, size: number, valuesPage: unknown[]): Node => {
      const remaining = budget();
      const rows = valuesPage.map((row) => cells(row).map((cell) => this.#preview(cell, remaining, path, 0, true)));
      if (failure && offset <= values.length && values.length < offset + size) rows.push([failure]);
      const total = values.length + Number(!done || failure !== undefined);
      const end = offset + rows.length;
      return { id: this.#id(), kind: "table", type: "Iterable", columns: columns.map((name) => ({ name: name.slice(0, MAX_TEXT) })),
        rows, total, totalKnown: done, next: end < total ? String(end) : null, handle };
    };
    const handle = this.#keep(async (offset, size) => page(offset, size, await read(offset, size)),
      async (offset, size) => {
        const rows = (await read(offset, size)).map((row) => cells(row).map(exportCell));
        if (failure) throw new Error(String(failure.message));
        const end = offset + rows.length;
        return { columns, rows, total: done ? values.length : null,
          next: !done || end < values.length ? String(end) : null };
      });
    return page(0, PAGE_SIZE, initial);
  }
}

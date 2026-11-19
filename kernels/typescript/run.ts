import { Console } from "node:console";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { Writable } from "node:stream";
import { StringDecoder } from "node:string_decoder";
import { dump, setDumpSink } from "./runtime.ts";

import { inspect, types } from "node:util";
import { parentPort, workerData } from "node:worker_threads";

export type Script = {
  runId: number;
  source: string;
  path: string | null;
  finalExpression: { start: number; end: number; append: boolean } | null;
};

const port = parentPort!;

const MAX_NODES = 1_000;
const MAX_DEPTH = 6;
const MAX_CHILDREN = 100;
const MAX_TEXT = 10_000;
const FINAL_DUMP_MODULE = "codingpad:internal";

function inspectValue(value: unknown) {
  return inspect(value, {
    customInspect: false, getters: false, depth: 2,
    maxArrayLength: MAX_CHILDREN, maxStringLength: MAX_TEXT,
  }).slice(0, MAX_TEXT);
}

function preview(value: unknown, runId: number, nextId: () => number) {
  let remaining = MAX_NODES;
  const ancestors = new Set<object>();

  function visit(value: unknown, depth: number): object {
    const id = `${runId}:n${nextId()}`;
    remaining--;
    switch (typeof value) {
      case "boolean":
        return { id, kind: "primitive", v: value };
      case "number":
        return Number.isFinite(value)
          ? { id, kind: "primitive", v: value }
          : { id, kind: "string", v: String(value) };
      case "string":
        return {
          id, kind: "string", v: value.slice(0, MAX_TEXT),
          truncated: value.length > MAX_TEXT, total: value.length,
        };
      case "undefined":
      case "bigint":
      case "symbol":
      case "function":
        return { id, kind: "string", v: inspectValue(value) };
    }
    if (value === null) return { id, kind: "null" };
    if (ancestors.has(value)) return { id, kind: "string", v: "[Circular]" };
    if (depth >= MAX_DEPTH) return { id, kind: "string", v: "[Depth limit reached]" };

    if (types.isDate(value) || types.isMap(value) || types.isSet(value)
      || types.isPromise(value) || types.isRegExp(value) || types.isArrayBufferView(value)
      || types.isProxy(value)) {
      return { id, kind: "string", v: inspectValue(value) };
    }

    ancestors.add(value);
    try {
      const read = (key: string) => {
        const property = Object.getOwnPropertyDescriptor(value, key);
        if (property && !("value" in property)) {
          remaining--;
          return { id: `${runId}:n${nextId()}`, kind: "string", v: "[Accessor]" };
        }
        return visit(property?.value, depth + 1);
      };
      if (Array.isArray(value)) {
        const items = [];
        for (let index = 0; index < Math.min(value.length, MAX_CHILDREN) && remaining > 0; index++) {
          items.push(read(String(index)));
        }
        return {
          id, kind: "list", type: "Array", items, next: null,
          total: value.length, truncated: items.length < value.length,
        };
      }
      const constructor = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(value) ?? {}, "constructor")?.value;
      const type = typeof constructor === "function" ? constructor.name || "Object" : "Object";
      const keys = Object.keys(value);
      const fields = [];
      for (const key of keys.slice(0, MAX_CHILDREN)) {
        if (remaining <= 0) break;
        fields.push({ name: key.slice(0, MAX_TEXT), value: read(key) });
      }
      return {
        id, kind: "object", type, fields, next: null,
        total: keys.length, truncated: fields.length < keys.length,
      };
    } finally {
      ancestors.delete(value);
    }
  }

  return visit(value, 0);
}

async function run({ runId, source, path: scriptPath, finalExpression }: Script) {
  const notify = (method: string, params: object) => port.postMessage({ method, params });
  const capture = (stream: "stdout" | "stderr") => {
    const decoder = new StringDecoder("utf8");
    const emit = (text: string) => {
      if (text) notify("stream", { stream, text });
    };
    const output = new Writable({
      write(chunk: Buffer, _encoding, done) {
        emit(decoder.write(chunk));
        done();
      },
    });
    return { output, flush: () => emit(decoder.end()) };
  };
  const stdout = capture("stdout");
  const stderr = capture("stderr");
  process.stdout.write = stdout.output.write.bind(stdout.output);
  process.stderr.write = stderr.output.write.bind(stderr.output);
  globalThis.console = new Console({ stdout: stdout.output, stderr: stderr.output });

  let nodeId = 0;
  setDumpSink((value) => notify("dump", { node: preview(value, runId, () => ++nodeId) }));
  const path = resolve(scriptPath ?? "scratch.ts");
  const contents = finalExpression
    ? withFinalDump(source, finalExpression)
    : source;

  // Match only the editor snapshot. Bun's runtime onLoad expects a result
  // for every match, so unrelated files must fall through before the hook.
  const escape = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const windows = process.platform === "win32";
  const pattern = windows ? path.split(/[\\/]/).map(escape).join("[\\\\/]") : escape(path);
  const filter = new RegExp(`^${pattern}$`, windows ? "i" : "");
  Bun.plugin({
    name: "CodingPad script",
    setup(builder) {
      builder.module("codingpad", () => ({ loader: "object", exports: { dump } }));
      builder.module(FINAL_DUMP_MODULE, () => ({
        loader: "object",
        exports: { dumpFinal: (value: unknown) => { if (value !== undefined) dump(value); } },
      }));
      builder.onResolve({ filter }, () => ({ path, namespace: "file" }));
      builder.onLoad({ filter }, () => ({ loader: "ts", contents }));
    },
  });
  let ok = true;
  try {
    await import(pathToFileURL(path).href);
  } catch (error) {
    ok = false;
    notify("stream", {
      stream: "stderr",
      text: (error instanceof Error ? error.message.slice(0, MAX_TEXT) : inspectValue(error)) + "\n",
    });
  }
  stdout.flush();
  stderr.flush();
  port.postMessage({ ok });
}

function withFinalDump(source: string, { start, end, append }: NonNullable<Script["finalExpression"]>) {
  const expression = source.slice(start, end);
  const wrapped = `;(await import("${FINAL_DUMP_MODULE}")).dumpFinal((${expression}))`;
  // Leave directive prologues in place. Only their literal value is appended.
  return append
    ? `${source}\n${wrapped};`
    : source.slice(0, start) + wrapped + source.slice(end);
}

port.ref();
await run(workerData as Script);

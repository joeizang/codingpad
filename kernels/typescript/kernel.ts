import { Console } from "node:console";
import { basename, resolve } from "node:path";
import { createInterface } from "node:readline";
import { pathToFileURL } from "node:url";
import { Writable } from "node:stream";
import { StringDecoder } from "node:string_decoder";
import { dump, setDumpSink } from "./runtime.ts";

type RunRequest = {
  id: number;
  params: { source: string; path: string | null };
};

const writeProtocol = process.stdout.write.bind(process.stdout);
const messages = createInterface({ input: process.stdin, crlfDelay: Infinity });

function send(message: object) {
  // Rust's JSON decoder requires strings to contain well-formed Unicode.
  writeProtocol(JSON.stringify({ jsonrpc: "2.0", ...message }, (_, value) =>
    typeof value === "string" ? value.toWellFormed() : value,
  ) + "\n");
}

function preview(value: unknown, runId: number, nextId: () => number) {
  let remaining = 500;
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
          id, kind: "string", v: value.slice(0, 10_000),
          truncated: value.length > 10_000, total: value.length,
        };
      case "undefined":
      case "bigint":
      case "symbol":
      case "function":
        return { id, kind: "string", v: Bun.inspect(value).slice(0, 10_000) };
    }
    if (value === null) return { id, kind: "null" };
    if (ancestors.has(value)) return { id, kind: "string", v: "[Circular]" };
    if (depth >= 5) return { id, kind: "string", v: "[Depth limit reached]" };

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
        for (let index = 0; index < Math.min(value.length, 50) && remaining > 0; index++) {
          items.push(read(String(index)));
        }
        return {
          id, kind: "list", type: "Array", items, next: null,
          total: value.length, truncated: items.length < value.length,
        };
      }
      const keys = Object.keys(value);
      const fields = [];
      for (const key of keys.slice(0, 50)) {
        if (remaining <= 0) break;
        fields.push({ name: key.slice(0, 10_000), value: read(key) });
      }
      return {
        id, kind: "object", type: "Object", fields, next: null,
        total: keys.length, truncated: fields.length < keys.length,
      };
    } finally {
      ancestors.delete(value);
    }
  }

  return visit(value, 0);
}

async function run({ id, params }: RunRequest) {
  const notify = (method: string, params: object) =>
    send({ method, params: { runId: id, ...params } });
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
  setDumpSink((value) => notify("dump", { node: preview(value, id, () => ++nodeId) }));
  const path = resolve(params.path || `.codingpad-${basename(import.meta.dir)}.ts`);

  // Load the editor snapshot as a virtual file at the script's path so relative
  // imports keep working and Run never needs to write to the script's folder.
  const fileName = basename(path).replace(/[.*+?^$(){}|[\]\\]/g, "\\$&");
  const filter = new RegExp(`[\\\\/]${fileName}$`);
  Bun.plugin({
    name: "CodingPad script",
    setup(builder) {
      builder.module("codingpad", () => ({ loader: "object", exports: { dump } }));
      builder.module("codingpad:internal", () => ({
        loader: "object",
        exports: { dumpFinal: (value: unknown) => { if (value !== undefined) dump(value); } },
      }));
      builder.onResolve({ filter }, (args) => {
        if (resolve(args.path) === path) return { path, namespace: "file" };
      });
      builder.onLoad({ filter }, (args) => {
        if (resolve(args.path) === path) return { loader: "ts", contents: params.source };
      });
    },
  });
  let ok = true;
  try {
    await import(pathToFileURL(path).href);
  } catch (error) {
    ok = false;
    notify("stream", {
      stream: "stderr",
      text: (error instanceof Error ? error.message : Bun.inspect(error)).slice(0, 10_000) + "\n",
    });
  }
  stdout.flush();
  stderr.flush();
  notify("run_finished", { ok });
  send({ id, result: { ok } });
}

// M5.0 runs one script per process. The supervisor owns process termination.
for await (const line of messages) {
  const request = JSON.parse(line) as RunRequest;
  try {
    await run(request);
  } catch (error) {
    send({ id: request.id, error: { code: -32603, message: String(error) } });
  }
  break;
}

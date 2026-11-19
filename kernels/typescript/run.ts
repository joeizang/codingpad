import { Console } from "node:console";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { Writable } from "node:stream";
import { StringDecoder } from "node:string_decoder";
import { cache, dump, html, image, live, setDumpSink } from "./runtime.ts";
import { Dumps, type PageRequest } from "./dump.ts";
import { errorDetails } from "./errors.ts";
import { parentPort, workerData } from "node:worker_threads";

export type Script = {
  runId: number;
  source: string;
  path: string | null;
  finalExpression: { start: number; end: number; append: boolean } | null;
};

const port = parentPort!;

const FINAL_DUMP_MODULE = "codingpad:internal";

async function run({ runId, source, path: scriptPath, finalExpression }: Script) {
  let running = true;
  const notify = (method: string, params: object) => {
    if (running) port.postMessage({ method, params });
  };
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
  globalThis.console = Object.assign(new Console({ stdout: stdout.output, stderr: stderr.output }), {
    write(...data: Parameters<typeof console.write>) {
      let bytes = 0;
      for (const value of data) {
        const chunk = typeof value === "string" ? Buffer.from(value) : Buffer.from(
          ArrayBuffer.isView(value) ? value.buffer : value,
          ArrayBuffer.isView(value) ? value.byteOffset : 0,
          value.byteLength,
        );
        stdout.output.write(chunk);
        bytes += chunk.length;
      }
      return bytes;
    },
  });

  const windows = process.platform === "win32";
  const resolvedPath = resolve(scriptPath ?? "scratch.ts");
  // Bun can normalize the drive letter when resolving a file URL.
  const path = windows ? resolvedPath.replace(/^[A-Z]:/, (drive) => drive.toLowerCase()) : resolvedPath;
  const prefix = `;(await import("${FINAL_DUMP_MODULE}")).dumpFinal((`;
  const location = (offset: number) => {
    const lines = source.slice(0, offset).split(/\r\n|[\n\r\u2028\u2029]/);
    return { line: lines.length, column: lines.at(-1)!.length };
  };
  const insertion = finalExpression && !finalExpression.append
    ? {
      ...location(finalExpression.start),
      length: prefix.length,
      endLine: location(finalExpression.end).line,
      endColumn: location(finalExpression.end).column
        + (location(finalExpression.end).line === location(finalExpression.start).line ? prefix.length : 0),
    }
    : undefined;
  const dumps = new Dumps(runId, notify, (error) => errorDetails(error, path, source, insertion));
  setDumpSink((value) => dumps.dump(value));
  const contents = finalExpression
    ? withFinalDump(source, finalExpression, prefix)
    : source;

  // Match only the editor snapshot. Bun's runtime onLoad expects a result
  // for every match, so unrelated files must fall through before the hook.
  const escape = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const pattern = windows ? path.split(/[\\/]/).map(escape).join("[\\\\/]") : escape(path);
  const filter = new RegExp(`^${pattern}$`, windows ? "i" : "");
  Bun.plugin({
    name: "CodingPad script",
    setup(builder) {
      builder.module("codingpad", () => ({ loader: "object", exports: { cache, dump, html, image, live } }));
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
    notify("dump", { node: dumps.error(error) });
  }
  await dumps.finish();
  stdout.flush();
  stderr.flush();
  running = false;

  if (dumps.canExpand) {
    port.on("message", async (request: { method: "dispose" } | {
      id: number; method: "expand" | "export_rows"; params: PageRequest;
    }) => {
      if (request.method === "dispose") {
        await dumps.dispose();
        port.postMessage({ disposed: true });
      } else {
        port.postMessage({ id: request.id, ...await dumps.page(request.params, request.method === "export_rows") });
      }
    });
  }
  port.postMessage({ ok, retain: dumps.canExpand });
}

function withFinalDump(source: string, { start, end, append }: NonNullable<Script["finalExpression"]>, prefix: string) {
  const expression = source.slice(start, end);
  const wrapped = `${prefix}${expression}))`;
  // Leave directive prologues in place. Only their literal value is appended.
  return append
    ? `${source}\n${wrapped};`
    : source.slice(0, start) + wrapped + source.slice(end);
}

port.ref();
await run(workerData as Script);

import { once } from "node:events";
import { createConnection } from "node:net";
import { createInterface } from "node:readline";
import { MessageChannel, Worker } from "node:worker_threads";
import type { MessagePort } from "node:worker_threads";
import { missingHandle, type PageRequest } from "./dump.ts";
import type { Script } from "./run.ts";

type Preview = { id: number; method: "expand" | "export_rows"; params: PageRequest };
type Request =
  | { id: number; method: "run"; params: Omit<Script, "runId"> }
  | { method: "cancel"; params: { runId: number } }
  | Preview;

const messages = createInterface({ input: process.stdin, crlfDelay: Infinity });
const requests = messages[Symbol.asyncIterator]();
const hello = await requests.next();
if (hello.done) process.exit(0);
const { port, token } = JSON.parse(hello.value) as { port: number; token: number[] };
const responses = createConnection({ host: "127.0.0.1", port });
await once(responses, "connect");
responses.setNoDelay(true);
responses.write(Buffer.from(token));

const workerUrl = new URL("./run.ts", import.meta.url);
let active: { id: number; stop: () => void } | undefined;
let retained: Worker | undefined;
let preview: Preview | undefined;

// Values persist for this kernel process. Reset and forced stop discard the
// process, which clears them; each run gets a fresh worker and clean globals.
const cache = new Map<string, unknown>();

type CacheRequest = {
  id: number;
  method: "get" | "set";
  key: string;
  value?: unknown;
};

function answerCache(request: CacheRequest, port: MessagePort, flag: Int32Array) {
  let reply: object;
  try {
    if (request.method === "get") {
      const found = cache.has(request.key);
      reply = { id: request.id, found, value: found ? cache.get(request.key) : undefined };
    } else {
      // postMessage already gave this process its own copy of the value, and
      // the get reply below clones it again for each later run.
      if (!cache.has(request.key)) cache.set(request.key, request.value);
      reply = { id: request.id, found: true };
    }
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    reply = { id: request.id, error: message };
  }
  try {
    port.postMessage(reply);
  } catch {
    // The worker was stopped while the value was being sent.
  } finally {
    // The worker waits until this exact request id appears, so a late reply
    // from an abandoned request cannot wake the next one early.
    Atomics.store(flag, 0, request.id);
    Atomics.notify(flag, 0);
  }
}

function send(message: object) {
  // Rust's JSON decoder requires strings to contain well-formed Unicode.
  responses.write(JSON.stringify({ jsonrpc: "2.0", ...message }, (_, value) =>
    typeof value === "string" ? value.toWellFormed() : value,
  ) + "\n");
}

function unavailable(id: number) {
  send({ id, error: { code: -32001, message: missingHandle } });
}

async function retire(worker: Worker) {
  // Give suspended generators a chance to release resources. A stuck
  // iterator or background callback must never hold up the next run.
  await new Promise<void>((resolve) => {
    const done = () => {
      clearTimeout(deadline);
      worker.off("message", reply);
      worker.off("exit", done);
      resolve();
    };
    const reply = (message: { disposed?: boolean }) => {
      if (message.disposed) done();
    };
    const deadline = setTimeout(done, 200);
    worker.on("message", reply);
    worker.once("exit", done);
    try { worker.postMessage({ method: "dispose" }); } catch { done(); }
  });
  void worker.terminate();
}

function run(id: number, params: Omit<Script, "runId">) {
  const { connection, sql_server_driver, secrets, secret_errors, ...script } = params;
  const notify = (method: string, params: object) => {
    if (active?.id === id) send({ method, params: { runId: id, ...params } });
  };
  const stderr = (text: string) => notify("stream", { stream: "stderr", text });
  const { port1: cachePort, port2: workerCachePort } = new MessageChannel();
  const cacheFlag = new SharedArrayBuffer(4);
  cachePort.on("message", (request: CacheRequest) =>
    answerCache(request, cachePort, new Int32Array(cacheFlag)));
  const worker = new Worker(workerUrl, {
    workerData: {
      ...script, runId: id, cachePort: workerCachePort, cacheFlag,
      credentials: { connection, sql_server_driver, secrets, secret_errors },
    },
    transferList: [workerCachePort],
    stdout: true, stderr: true,
  });
  let stopping = false;
  let outcome: boolean | undefined;
  const finish = (ok: boolean) => {
    notify("run_finished", { ok });
    active = undefined;
    send({ id, result: { ok } });
  };
  active = { id, stop: () => {
    stopping = true;
    // Stop can reach the kernel even when the script has blocked its event loop.
    // Wait for exit before accepting another run. The host owns the deadline.
    void worker.terminate();
  } };

  worker.stdout.setEncoding("utf8").on("data", (text: string) =>
    notify("stream", { stream: "stdout", text }));
  worker.stderr.setEncoding("utf8").on("data", stderr);
  worker.on("message", (message) => {
    if ("ok" in message) {
      if (active?.id !== id || stopping) return;
      outcome = message.ok;
      if (message.retain) {
        retained = worker;
        finish(message.ok);
      } else {
        void worker.terminate();
      }
    } else if ("id" in message) {
      if (retained === worker && preview?.id === message.id) {
        preview = undefined;
        send(message);
      }
    } else if ("method" in message) {
      notify(message.method, message.params);
    }
  });
  worker.on("error", (error) => {
    if (active?.id === id && !stopping) stderr(String(error) + "\n");
    void worker.terminate();
  });
  worker.on("exit", (code) => {
    cachePort.close();
    if (retained === worker) {
      retained = undefined;
      if (preview) unavailable(preview.id);
      preview = undefined;
    }
    if (active?.id !== id) return;
    if (!stopping && outcome === undefined) stderr(`TypeScript worker exited before completing the run (${code}).\n`);
    finish(outcome ?? false);
  });
}

for (let next = await requests.next(); !next.done; next = await requests.next()) {
  const request = JSON.parse(next.value) as Request;
  if (request.method === "cancel") {
    if (active?.id === request.params.runId) active.stop();
  } else if (request.method === "run") {
    if (active || preview) {
      send({ id: request.id, error: { code: -32000, message: "Run already in progress" } });
      continue;
    }
    const previous = retained;
    retained = undefined;
    if (previous) await retire(previous);
    try {
      run(request.id, request.params);
    } catch (error) {
      send({ id: request.id, error: { code: -32603, message: String(error) } });
    }
  } else if (request.method === "expand" || request.method === "export_rows") {
    if (!retained || preview) {
      unavailable(request.id);
      continue;
    }
    preview = request;
    try {
      retained.postMessage(request);
    } catch {
      preview = undefined;
      unavailable(request.id);
    }
  } else {
    send({ id: (request as { id: number }).id, error: { code: -32601, message: "Method unavailable" } });
  }
}
active?.stop();
if (retained) void retained.terminate();
active = undefined;
retained = undefined;
responses.end();

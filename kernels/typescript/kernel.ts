import { createInterface } from "node:readline";
import { Worker } from "node:worker_threads";
import type { Script } from "./run.ts";

type Request =
  | { id: number; method: "run"; params: Omit<Script, "runId"> }
  | { method: "cancel"; params: { runId: number } }
  | { id: number; method: "expand" };

const messages = createInterface({ input: process.stdin, crlfDelay: Infinity });
const workerUrl = new URL("./run.ts", import.meta.url);
let active: { id: number; stop: () => void } | undefined;

function send(message: object) {
  // Rust's JSON decoder requires strings to contain well-formed Unicode.
  process.stdout.write(JSON.stringify({ jsonrpc: "2.0", ...message }, (_, value) =>
    typeof value === "string" ? value.toWellFormed() : value,
  ) + "\n");
}

function run(id: number, params: Omit<Script, "runId">) {
  const notify = (method: string, params: object) => {
    if (active?.id === id) send({ method, params: { runId: id, ...params } });
  };
  const stderr = (text: string) => notify("stream", { stream: "stderr", text });
  const worker = new Worker(workerUrl, {
    workerData: { ...params, runId: id }, stdout: true, stderr: true,
  });
  let outcome: boolean | undefined;
  const finish = (ok: boolean) => {
    if (outcome !== undefined) return;
    outcome = ok;
    // Stop can reach the kernel even when the script has blocked its event loop.
    // Wait for exit before accepting another run. The host owns the deadline.
    void worker.terminate();
  };
  active = { id, stop: () => finish(false) };

  worker.stdout.setEncoding("utf8").on("data", (text: string) =>
    notify("stream", { stream: "stdout", text }));
  worker.stderr.setEncoding("utf8").on("data", stderr);
  worker.on("message", (message) => {
    if ("ok" in message) finish(message.ok);
    else notify(message.method, message.params);
  });
  worker.on("error", (error) => {
    if (outcome === undefined) stderr(error.message + "\n");
    finish(false);
  });
  worker.on("exit", (code) => {
    if (outcome === undefined) stderr(`TypeScript worker exited before completing the run (${code}).\n`);
    const ok = outcome ?? false;
    notify("run_finished", { ok });
    active = undefined;
    send({ id, result: { ok } });
  });
}

for await (const line of messages) {
  const request = JSON.parse(line) as Request;
  if (request.method === "cancel") {
    if (active?.id === request.params.runId) active.stop();
  } else if (request.method === "run" && !active) {
    try {
      run(request.id, request.params);
    } catch (error) {
      send({ id: request.id, error: { code: -32603, message: String(error) } });
    }
  } else {
    send({ id: request.id, error: { code: -32601, message: "Method unavailable" } });
  }
}
active?.stop();

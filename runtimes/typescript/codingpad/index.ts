import { closeSync, openSync, readSync } from "node:fs";
import { basename, extname } from "node:path";
import { serialize } from "node:v8";
import { receiveMessageOnPort, workerData } from "node:worker_threads";
import type { MessagePort } from "node:worker_threads";

let sink: ((value: unknown) => void) | undefined;

export function dump(value: unknown): void {
  if (sink) sink(value);
  else console.log(value instanceof Html ? value.markup : value);
}

export function setDumpSink(send: (value: unknown) => void): void {
  sink = send;
}

// The kernel owns the cache so values survive reruns. The worker asks for a
// key synchronously over a MessageChannel; Atomics.wait wakes it after the
// kernel has queued the reply, which receiveMessageOnPort then reads.
const channel = workerData as
  | { cachePort?: MessagePort; cacheFlag?: SharedArrayBuffer }
  | null
  | undefined;
const cachePort = channel?.cachePort;
const cacheFlag = channel?.cacheFlag ? new Int32Array(channel.cacheFlag) : undefined;
const cacheTimeout = 10_000;

type CacheReply =
  | { id: number; found: boolean; value?: unknown; error?: never }
  | { id: number; error: string; found?: never; value?: never };

const runCache = new Map<string, unknown>();
let nextRequest = 0;

// Read queued replies, discarding any left over from an earlier timed-out
// request. The kernel sends replies in request order, so the wanted one is
// the last matching message.
function receiveReply(id?: number): CacheReply | undefined {
  for (;;) {
    const received = receiveMessageOnPort(cachePort!);
    if (!received) return undefined;
    const reply = received.message as CacheReply;
    if (reply.id === id) return reply;
  }
}

function askCache(request: { method: "get" | "set"; key: string; value?: unknown }): CacheReply {
  if (!cachePort || !cacheFlag) throw new Error("cache() is unavailable outside a CodingPad kernel");
  receiveReply();
  const id = ++nextRequest;
  try {
    cachePort.postMessage({ ...request, id });
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error);
    throw new TypeError(`cache() values must be structured-cloneable: ${detail}`);
  }
  const deadline = Date.now() + cacheTimeout;
  // The flag holds the id whose reply is queued. A late reply can wake this
  // waiter with an older id, so keep waiting until the current one arrives.
  // Read the flag once per iteration: the kernel writes it from another
  // thread, and a second read could wait for a value that has already arrived.
  for (;;) {
    const seen = Atomics.load(cacheFlag, 0);
    if (seen === id) break;
    const remaining = deadline - Date.now();
    if (remaining <= 0) {
      receiveReply();
      throw new Error("cache() timed out waiting for the CodingPad kernel");
    }
    Atomics.wait(cacheFlag, 0, seen, remaining);
  }
  const reply = receiveReply(id);
  if (!reply) {
    throw new Error("cache() received no reply from the CodingPad kernel");
  }
  return reply;
}

function cacheKey(key: unknown): string {
  try {
    return serialize(key).toString("base64");
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error);
    throw new TypeError(`cache() keys must be structured-cloneable values: ${detail}`);
  }
}

function cloneValue<T>(value: T): T {
  try {
    return structuredClone(value);
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error);
    throw new TypeError(`cache() values must be structured-cloneable: ${detail}`);
  }
}

export function cache<T>(key: unknown, factory: () => T): T {
  const id = cacheKey(key);
  if (runCache.has(id)) return runCache.get(id) as T;

  if (cachePort && cacheFlag) {
    const reply = askCache({ method: "get", key: id });
    if (reply.error) throw new Error(`cache() could not read the value: ${reply.error}`);
    if (reply.found) {
      runCache.set(id, reply.value);
      return reply.value as T;
    }
  }

  const factoryValue = factory();
  if (factoryValue !== null && (typeof factoryValue === "object" || typeof factoryValue === "function")
    && typeof (factoryValue as { then?: unknown }).then === "function") {
    void Promise.resolve(factoryValue).catch(() => {});
    throw new TypeError("cache() factories must return synchronously; await async work before calling cache()");
  }

  // Clone on a miss too, so the first run sees the same kind of value as
  // every later run. Prototypes and accessors do not survive structured clone.
  const value = cloneValue(factoryValue);
  if (cachePort && cacheFlag) {
    const reply = askCache({ method: "set", key: id, value });
    if (reply.error) throw new Error(`cache() could not keep the value: ${reply.error}`);
  }
  // The first stored value wins. A single worker never runs two cache misses
  // for one key concurrently, so this run keeps the value it computed.
  if (!runCache.has(id)) runCache.set(id, value);
  return runCache.get(id) as T;
}

export class Html {
  constructor(readonly markup: string) {}
}

export function html(markup: string): Html {
  return new Html(markup.slice(0, 200_000));
}

export class Live<T> {
  #value: T;
  readonly listeners = new Set<(value: T) => void>();

  constructor(value: T) {
    this.#value = value;
  }

  get value(): T {
    return this.#value;
  }

  set value(value: T) {
    this.#value = value;
    for (const listener of this.listeners) listener(value);
  }
}

export function live<T>(value: T): Live<T> {
  return new Live(value);
}

export class Image {
  constructor(readonly src: string, readonly mime: string, readonly name?: string) {}
}

const imageTypes = new Map([
  [".png", "image/png"], [".jpg", "image/jpeg"], [".jpeg", "image/jpeg"],
  [".gif", "image/gif"], [".webp", "image/webp"], [".bmp", "image/bmp"],
  [".svg", "image/svg+xml"],
]);
const MAX_IMAGE_BYTES = 4 * 1024 * 1024;

function imageMime(bytes: Buffer, name?: string) {
  if (bytes.subarray(0, 8).equals(Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]))) return "image/png";
  if (bytes.subarray(0, 3).equals(Buffer.from([255, 216, 255]))) return "image/jpeg";
  const header = bytes.toString("ascii", 0, 12);
  if (header.startsWith("GIF87a") || header.startsWith("GIF89a")) return "image/gif";
  if (header.startsWith("RIFF") && header.endsWith("WEBP")) return "image/webp";
  if (header.startsWith("BM")) return "image/bmp";
  if (/^\s*(?:<\?xml[^>]*>\s*)?<svg\b/i.test(bytes.toString("utf8", 0, 512))) return "image/svg+xml";
  return name ? imageTypes.get(extname(name).toLowerCase()) : undefined;
}

export function image(source: string | Uint8Array | ArrayBuffer, mime?: string): Image {
  let bytes: Buffer;
  let name: string | undefined;
  if (typeof source === "string") {
    name = basename(source);
    const file = openSync(source, "r");
    try {
      bytes = Buffer.allocUnsafe(MAX_IMAGE_BYTES + 1);
      let length = 0;
      while (length < bytes.length) {
        const read = readSync(file, bytes, length, bytes.length - length, null);
        if (read === 0) break;
        length += read;
      }
      bytes = bytes.subarray(0, length);
    } finally {
      closeSync(file);
    }
  } else {
    if (source.byteLength > MAX_IMAGE_BYTES) throw new RangeError("Images are limited to 4 MiB");
    bytes = source instanceof ArrayBuffer
      ? Buffer.from(source)
      : Buffer.from(source.buffer, source.byteOffset, source.byteLength);
  }
  if (bytes.length > MAX_IMAGE_BYTES) throw new RangeError("Images are limited to 4 MiB");
  const type = mime ?? imageMime(bytes, name);
  if (!type || ![...imageTypes.values()].includes(type)) throw new TypeError("Unknown image format");
  return new Image(`data:${type};base64,${bytes.toString("base64")}`, type, name);
}

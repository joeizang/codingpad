import { closeSync, openSync, readSync } from "node:fs";
import { basename, extname } from "node:path";

let sink: ((value: unknown) => void) | undefined;

export function dump(value: unknown): void {
  if (sink) sink(value);
  else console.log(value instanceof Html ? value.markup : value);
}

export function setDumpSink(send: (value: unknown) => void): void {
  sink = send;
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

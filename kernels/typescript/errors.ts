import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { inspect } from "node:util";

export const MAX_TEXT = 10_000;

export function inspectValue(value: unknown) {
  return inspect(value, {
    customInspect: false, getters: false, depth: 2,
    maxArrayLength: 100, maxStringLength: MAX_TEXT,
  }).slice(0, MAX_TEXT);
}

export function errorDetails(
  error: unknown,
  path: string,
  source: string,
  insertion?: { line: number; column: number; length: number; endLine: number; endColumn: number },
) {
  const lines = source.split(/\r\n|[\n\r\u2028\u2029]/);
  const frames = [];
  function frame(file: string, line: number, column: number, name = "") {
    if (file.startsWith("file:")) {
      try { file = fileURLToPath(file); } catch { /* Keep Bun's path if the URL is malformed. */ }
    }
    const samePath = process.platform === "win32"
      ? resolve(file).toLowerCase() === path.toLowerCase()
      : resolve(file) === path;
    if (samePath && insertion) {
      if (line === insertion.endLine && column > insertion.endColumn) column = Math.max(insertion.endColumn, column - 2);
      if (line === insertion.line && column > insertion.column) column = Math.max(insertion.column + 1, column - insertion.length);
    }
    const script = samePath && line >= 1 && line <= lines.length;
    return { file, line, column, function: name, text: script ? lines[line - 1].slice(0, MAX_TEXT) : "", script };
  }

  function describe(value: unknown) {
    const detail = value as { name?: unknown; message?: unknown } | null;
    return {
      type: typeof detail?.name === "string" ? detail.name.slice(0, MAX_TEXT) : "Thrown value",
      message: typeof detail?.message === "string" ? detail.message.slice(0, MAX_TEXT) : inspectValue(value),
    };
  }

  try {
    const detail = error as {
      stack?: unknown;
      errors?: unknown[];
      position?: { file: string; line: number; column: number };
      cause?: unknown;
    } | null;
    const diagnostics = Array.isArray(detail?.errors) ? detail.errors.slice(0, 20) : [detail];
    for (const diagnostic of diagnostics) {
      const position = (diagnostic as typeof detail)?.position;
      if (position && typeof position.file === "string" && position.line > 0) {
        frames.push(frame(position.file, position.line, position.column));
      }
    }
    if (typeof detail?.stack === "string") {
      for (const entry of detail.stack.split("\n").slice(0, 51)) {
        // Bun uses "at name (file:line:column)" and "at file:line:column".
        const match = /^\s+at (?:(.*?) \()?(.+):(\d+):(\d+)\)?$/.exec(entry);
        if (match) frames.push(frame(match[2], Number(match[3]), Number(match[4]), match[1]));
      }
    }
    const causes = [];
    let cause = detail?.cause;
    while (cause !== undefined && causes.length < 5) {
      causes.push(describe(cause));
      cause = (cause as typeof detail)?.cause;
    }
    return { ...describe(error), traceback: frames, causes };
  } catch {
    return { type: "Error", message: "Could not inspect thrown value", traceback: [], causes: [] };
  }
}

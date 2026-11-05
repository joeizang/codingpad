export type CompileDiagnostic = {
  message: string;
  line: number;
  column: number;
  endLine: number;
  endColumn: number;
};

export type TraceFrame = {
  file: string;
  line: number;
  column: number;
  function: string;
  text: string;
  script: boolean;
};

export type DumpNode =
  | { id?: string; kind: "null" }
  | { id?: string; kind: "primitive"; v: number | boolean }
  | {
      id?: string;
      kind: "string";
      v: string;
      type?: string;
      truncated?: boolean;
      total?: number;
    }
  | {
      id?: string;
      kind: "list";
      type: string;
      items: DumpNode[];
      total?: number;
      next?: string | null;
      handle?: string;
      truncated: boolean;
    }
  | {
      id?: string;
      kind: "object";
      type: string;
      fields: { name: string; value: DumpNode }[];
      total?: number;
      next?: string | null;
      handle?: string;
      truncated: boolean;
    }
  | {
      id?: string;
      kind: "lazy";
      type?: string;
      preview: string;
      handle: string;
    }
  | {
      id?: string;
      kind: "table";
      type: string;
      columns: { name: string; type?: string }[];
      rows: DumpNode[][];
      total: number;
      totalKnown?: boolean;
      truncated?: boolean;
      next: string | null;
      handle?: string;
    }
  | { id?: string; kind: "image"; mime: string; src: string; name?: string }
  | { id?: string; kind: "html"; html: string }
  | { id?: string; kind: "live"; node: DumpNode }
  | {
      id?: string;
      kind: "error";
      type: string;
      message: string;
      traceback: TraceFrame[];
      causes?: { type: string; message: string }[];
    };

export type KernelEvent =
  | { method: "diagnostics"; params: { runId: number; diagnostics: CompileDiagnostic[] } }
  | {
      method: "stream";
      params: { runId: number; stream: "stdout" | "stderr"; text: string };
    }
  | { method: "dump"; params: { runId: number; node: DumpNode } }
  | {
      method: "dump_update";
      params: { runId: number; nodeId: string; node: DumpNode };
    }
  | { method: "run_finished"; params: { runId: number; ok: boolean } };

export type Output = { id: number } & (
  | { kind: "stream"; stream: "stdout" | "stderr"; text: string }
  | { kind: "dump"; node: DumpNode }
);

// Coalesce a frame's fragments with one join per stream block. Repeatedly
// appending each fragment to a 100 KB string would still be quadratic in a batch.
export function appendOutputBatch(
  output: Output[],
  events: KernelEvent[],
): Output[] {
  let nextId = (output.at(-1)?.id ?? -1) + 1;
  const incoming: Output[] = [];
  let stream: "stdout" | "stderr" | undefined;
  let fragments: string[] = [];
  const flush = () => {
    if (stream !== undefined) {
      incoming.push({
        id: nextId++,
        kind: "stream",
        stream,
        text: fragments.join("").slice(-100000),
      });
      stream = undefined;
      fragments = [];
    }
  };
  for (const event of events) {
    if (event.method === "run_finished" || event.method === "dump_update" || event.method === "diagnostics")
      continue;
    if (event.method === "dump") {
      flush();
      incoming.push({ id: nextId++, kind: "dump", node: event.params.node });
    } else {
      if (stream !== event.params.stream) flush();
      stream = event.params.stream;
      fragments.push(event.params.text);
    }
  }
  flush();
  if (!incoming.length) return output;
  let previous = output;
  const last = output.at(-1);
  const first = incoming[0];
  if (
    last?.kind === "stream" &&
    first.kind === "stream" &&
    last.stream === first.stream
  ) {
    previous = output.slice(0, -1);
    incoming[0] = { ...last, text: (last.text + first.text).slice(-100000) };
  }
  return [...previous, ...incoming].slice(-500);
}

export function appendOutput(output: Output[], event: KernelEvent): Output[] {
  return appendOutputBatch(output, [event]);
}

export function collectDumpUpdates(
  events: KernelEvent[],
): Record<string, DumpNode> {
  const updates: Record<string, DumpNode> = {};
  for (const event of events) {
    if (event.method === "dump_update") {
      updates[event.params.nodeId] = event.params.node;
    }
  }
  return updates;
}

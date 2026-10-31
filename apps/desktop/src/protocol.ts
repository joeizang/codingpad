export type DumpNode =
  | { kind: "null" }
  | { kind: "primitive"; v: number | boolean }
  | { kind: "string"; v: string; type?: string }
  | { kind: "list"; type: string; items: DumpNode[]; truncated: boolean }
  | {
      kind: "object";
      type: string;
      fields: { name: string; value: DumpNode }[];
      truncated: boolean;
    };

export type KernelEvent =
  | {
      method: "stream";
      params: { runId: number; stream: "stdout" | "stderr"; text: string };
    }
  | { method: "dump"; params: { runId: number; node: DumpNode } }
  | { method: "run_finished"; params: { runId: number; ok: boolean } };

export type Output =
  | { kind: "stream"; stream: "stdout" | "stderr"; text: string }
  | { kind: "dump"; node: DumpNode };

// Merge adjacent print fragments, while keeping dumps and stderr in order.
// Bound output so a verbose script cannot grow the UI indefinitely.
export function appendOutput(output: Output[], event: KernelEvent): Output[] {
  if (event.method === "run_finished") return output;
  if (event.method === "dump")
    return [...output.slice(-499), { kind: "dump", node: event.params.node }];
  const { stream, text } = event.params;
  const last = output.at(-1);
  if (last?.kind === "stream" && last.stream === stream) {
    return [
      ...output.slice(0, -1),
      { kind: "stream", stream, text: (last.text + text).slice(-100000) },
    ];
  }
  return [
    ...output.slice(-499),
    { kind: "stream", stream, text: text.slice(-100000) },
  ];
}

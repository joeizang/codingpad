import { expect, test } from "vitest";
import { appendOutput, type KernelEvent } from "./protocol";

test("streams coalesce without losing interleaved dumps or stderr", () => {
  const events: KernelEvent[] = [
    { method: "stream", params: { runId: 1, stream: "stdout", text: "hello" } },
    { method: "stream", params: { runId: 1, stream: "stdout", text: "\n" } },
    { method: "dump", params: { runId: 1, node: { kind: "null" } } },
    { method: "stream", params: { runId: 1, stream: "stderr", text: "error" } },
  ];
  expect(events.reduce(appendOutput, [])).toEqual([
    { kind: "stream", stream: "stdout", text: "hello\n" },
    { kind: "dump", node: { kind: "null" } },
    { kind: "stream", stream: "stderr", text: "error" },
  ]);
});

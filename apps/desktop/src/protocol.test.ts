import { expect, test } from "vitest";
import { appendOutput, appendOutputBatch, type KernelEvent } from "./protocol";

test("streams coalesce without losing interleaved dumps or stderr", () => {
  const events: KernelEvent[] = [
    { method: "stream", params: { runId: 1, stream: "stdout", text: "hello" } },
    { method: "stream", params: { runId: 1, stream: "stdout", text: "\n" } },
    { method: "dump", params: { runId: 1, node: { kind: "null" } } },
    { method: "stream", params: { runId: 1, stream: "stderr", text: "error" } },
  ];
  expect(events.reduce(appendOutput, [])).toEqual([
    { id: 0, kind: "stream", stream: "stdout", text: "hello\n" },
    { id: 1, kind: "dump", node: { kind: "null" } },
    { id: 2, kind: "stream", stream: "stderr", text: "error" },
  ]);
});

test("batching bounds verbose output and retains the final text", () => {
  const events: KernelEvent[] = Array.from({ length: 100000 }, (_, index) => ({
    method: "stream",
    params: { runId: 1, stream: "stdout", text: `${index}\n` },
  }));
  const output = appendOutputBatch([], events);
  expect(output).toHaveLength(1);
  expect(output[0]).toMatchObject({
    kind: "stream",
    text: expect.stringMatching(/99999\n$/),
  });
  if (output[0].kind === "stream") expect(output[0].text).toHaveLength(100000);
});

test("dropping old blocks and merging streams preserve retained output ids", () => {
  const dump: KernelEvent = {
    method: "dump",
    params: { runId: 1, node: { kind: "null" } },
  };
  const before = appendOutputBatch(
    [],
    Array.from({ length: 500 }, () => dump),
  );
  const after = appendOutput(before, dump);
  expect(after).toHaveLength(500);
  expect(after[0].id).toBe(before[1].id);
  expect(after.at(-1)!.id).toBeGreaterThan(before.at(-1)!.id);
  const stream: KernelEvent = {
    method: "stream",
    params: { runId: 1, stream: "stdout", text: "hello" },
  };
  const first = appendOutput(after, stream);
  expect(appendOutput(first, stream).at(-1)!.id).toBe(first.at(-1)!.id);
});

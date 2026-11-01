import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import App from "./App";
import type { KernelEvent } from "./protocol";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({
  invoke,
  Channel: class {
    onmessage = (_: KernelEvent) => {};
  },
}));
vi.mock("./CodeEditor", () => ({
  default: ({
    source,
    onChange,
  }: {
    source: string;
    onChange: (source: string) => void;
  }) => (
    <textarea
      aria-label="Code"
      value={source}
      onChange={(e) => onChange(e.target.value)}
    />
  ),
}));
beforeEach(() => {
  invoke.mockReset();
});

test("Run renders streamed text, structured dumps and completion", async () => {
  invoke.mockImplementation(async (_command, args) => {
    args.onEvent.onmessage({
      method: "stream",
      params: { runId: 1, stream: "stdout", text: "Hello!\n" },
    });
    args.onEvent.onmessage({
      method: "dump",
      params: {
        runId: 1,
        node: {
          kind: "object",
          type: "dict",
          fields: [{ name: "answer", value: { kind: "primitive", v: 42 } }],
          truncated: false,
        },
      },
    });
    return { ok: true };
  });
  render(<App />);
  fireEvent.click(screen.getByRole("button", { name: "▶ Run" }));
  await screen.findByText("Finished");
  expect(screen.getByText("Hello!")).toBeInTheDocument();
  expect(screen.getByText("answer")).toBeInTheDocument();
  expect(screen.getByText("42")).toBeInTheDocument();
  expect(invoke).toHaveBeenCalledWith(
    "run_script",
    expect.objectContaining({
      scriptId: "scratch-1",
      source: expect.stringContaining("from codingpad import dump"),
    }),
  );
});

test("startup failure is visible and Run becomes available again", async () => {
  invoke.mockRejectedValue(
    "uv was not found. Install uv and restart CodingPad.",
  );
  render(<App />);
  fireEvent.click(screen.getByRole("button", { name: "▶ Run" }));
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "uv was not found",
  );
  await waitFor(() =>
    expect(screen.getByRole("button", { name: "▶ Run" })).toBeEnabled(),
  );
});

test("tabs preserve source and close their kernel", async () => {
  invoke.mockResolvedValue(undefined);
  render(<App />);
  fireEvent.change(screen.getByLabelText("Code"), {
    target: { value: "a = 1" },
  });
  fireEvent.click(screen.getByRole("button", { name: "New scratch script" }));
  expect(screen.getByLabelText("Code")).not.toHaveValue("a = 1");
  fireEvent.click(screen.getByRole("button", { name: "Scratch 1.py" }));
  expect(screen.getByLabelText("Code")).toHaveValue("a = 1");
  fireEvent.click(screen.getByRole("button", { name: "Close Scratch 1.py" }));
  await waitFor(() =>
    expect(
      screen.queryByRole("button", { name: "Scratch 1.py" }),
    ).not.toBeInTheDocument(),
  );
  expect(invoke).toHaveBeenCalledWith("close_script", {
    scriptId: "scratch-1",
  });
});

test("running script cannot be submitted twice", async () => {
  let finish!: (value: { ok: boolean }) => void;
  invoke.mockReturnValue(
    new Promise((resolve) => {
      finish = resolve;
    }),
  );
  render(<App />);
  fireEvent.click(screen.getByRole("button", { name: "▶ Run" }));
  expect(screen.getByRole("button", { name: "Running…" })).toBeDisabled();
  finish({ ok: true });
  await screen.findByText("Finished");
  expect(invoke).toHaveBeenCalledTimes(1);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

test("many stream fragments flush once per animation frame and at completion", async () => {
  let callback!: FrameRequestCallback;
  const schedule = vi.fn((next: FrameRequestCallback) => {
    callback = next;
    return 1;
  });
  vi.stubGlobal("requestAnimationFrame", schedule);
  vi.stubGlobal("cancelAnimationFrame", vi.fn());
  let channel!: { onmessage: (event: KernelEvent) => void };
  let finish!: (value: { ok: boolean }) => void;
  invoke.mockImplementation((_command, args) => {
    channel = args.onEvent;
    return new Promise((resolve) => {
      finish = resolve;
    });
  });
  render(<App />);
  fireEvent.click(screen.getByRole("button", { name: "▶ Run" }));
  act(() => {
    for (let index = 0; index < 1000; index++) {
      channel.onmessage({
        method: "stream",
        params: { runId: 1, stream: "stdout", text: `${index}\n` },
      });
    }
  });
  expect(schedule).toHaveBeenCalledTimes(1);
  expect(screen.queryByText(/999/)).not.toBeInTheDocument();
  act(() => callback(0));
  expect(screen.getByText(/999/)).toBeInTheDocument();
  await act(async () => {
    channel.onmessage({
      method: "stream",
      params: { runId: 1, stream: "stdout", text: "final fragment" },
    });
    finish({ ok: true });
  });
  expect(screen.getByText(/final fragment/)).toBeInTheDocument();
  expect(screen.getByText("Finished")).toBeInTheDocument();
});

test("collapsed dump stays attached to its output when old blocks are dropped", async () => {
  let channel!: { onmessage: (event: KernelEvent) => void };
  invoke.mockImplementation(async (_command, args) => {
    channel = args.onEvent;
    for (let index = 0; index < 500; index++) {
      channel.onmessage({
        method: "dump",
        params: {
          runId: 1,
          node:
            index === 10
              ? {
                  kind: "list",
                  type: "keep collapsed",
                  items: [],
                  truncated: false,
                }
              : { kind: "primitive", v: index },
        },
      });
    }
    return { ok: true };
  });
  render(<App />);
  fireEvent.click(screen.getByRole("button", { name: "▶ Run" }));
  await screen.findByText("Finished");
  const details = screen.getByText(/keep collapsed/).closest("details")!;
  fireEvent.click(details.querySelector("summary")!);
  expect(details.open).toBe(false);
  act(() =>
    channel.onmessage({
      method: "dump",
      params: { runId: 1, node: { kind: "primitive", v: 500 } },
    }),
  );
  await screen.findByText("500");
  expect(screen.getByText(/keep collapsed/).closest("details")).toBe(details);
  expect(details.open).toBe(false);
});

test("late events from an earlier run cannot contaminate a rerun", async () => {
  const channels: { onmessage: (event: KernelEvent) => void }[] = [];
  invoke.mockImplementation(async (_command, args) => {
    channels.push(args.onEvent);
    return { ok: true };
  });
  render(<App />);
  fireEvent.click(screen.getByRole("button", { name: "▶ Run" }));
  await screen.findByText("Finished");
  fireEvent.click(screen.getByRole("button", { name: "▶ Run" }));
  await waitFor(() => expect(invoke).toHaveBeenCalledTimes(2));
  await act(async () => {
    channels[0].onmessage({
      method: "stream",
      params: { runId: 1, stream: "stdout", text: "stale output" },
    });
    channels[1].onmessage({
      method: "stream",
      params: { runId: 2, stream: "stdout", text: "current output" },
    });
  });
  await screen.findByText("current output");
  expect(screen.queryByText("stale output")).not.toBeInTheDocument();
});

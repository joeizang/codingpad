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
const dialog = vi.hoisted(() => ({
  open: vi.fn(),
  save: vi.fn(),
  ask: vi.fn(),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: dialog.open,
  save: dialog.save,
  ask: dialog.ask,
}));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    onCloseRequested: vi.fn().mockResolvedValue(() => {}),
  }),
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
  dialog.open.mockReset();
  dialog.save.mockReset();
  dialog.ask.mockReset();
  dialog.ask.mockResolvedValue(true);
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
  expect(
    invoke.mock.calls.filter(([command]) => command === "run_script"),
  ).toHaveLength(1);
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
  invoke.mockImplementation((command, args) => {
    if (command === "load_library_settings") return Promise.resolve(undefined);
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
  invoke.mockImplementation(async (command, args) => {
    if (command === "load_library_settings") return undefined;
    channels.push(args.onEvent);
    return { ok: true };
  });
  render(<App />);
  fireEvent.click(screen.getByRole("button", { name: "▶ Run" }));
  await screen.findByText("Finished");
  fireEvent.click(screen.getByRole("button", { name: "▶ Run" }));
  await waitFor(() =>
    expect(
      invoke.mock.calls.filter(([command]) => command === "run_script"),
    ).toHaveLength(2),
  );
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

test("live dump updates replace their inner block and keep the badge", async () => {
  invoke.mockImplementation(async (_command, args) => {
    args.onEvent.onmessage({
      method: "dump",
      params: {
        runId: 1,
        node: {
          id: "wrapper",
          kind: "live",
          node: { id: "inner", kind: "primitive", v: 1 },
        },
      },
    });
    args.onEvent.onmessage({
      method: "dump_update",
      params: {
        runId: 1,
        nodeId: "inner",
        node: { id: "inner-2", kind: "primitive", v: 2 },
      },
    });
    return { ok: true };
  });
  render(<App />);
  fireEvent.click(screen.getByRole("button", { name: "▶ Run" }));
  await screen.findByText("Finished");
  expect(screen.getByText("2")).toBeInTheDocument();
  expect(screen.queryByText("1")).not.toBeInTheDocument();
  expect(screen.getByText("live")).toBeInTheDocument();
});

test("live updates share the animation-frame batch instead of re-rendering each", async () => {
  let callback!: FrameRequestCallback;
  const schedule = vi.fn((next: FrameRequestCallback) => {
    callback = next;
    return 1;
  });
  vi.stubGlobal("requestAnimationFrame", schedule);
  vi.stubGlobal("cancelAnimationFrame", vi.fn());
  let channel!: { onmessage: (event: KernelEvent) => void };
  let finish!: (value: { ok: boolean }) => void;
  invoke.mockImplementation((command, args) => {
    if (command === "load_library_settings") return Promise.resolve(undefined);
    channel = args.onEvent;
    return new Promise((resolve) => {
      finish = resolve;
    });
  });
  render(<App />);
  fireEvent.click(screen.getByRole("button", { name: "▶ Run" }));
  act(() => {
    channel.onmessage({
      method: "dump",
      params: {
        runId: 1,
        node: {
          id: "wrapper",
          kind: "live",
          node: { id: "inner", kind: "primitive", v: 0 },
        },
      },
    });
    for (let index = 1; index <= 100; index++) {
      channel.onmessage({
        method: "dump_update",
        params: {
          runId: 1,
          nodeId: "inner",
          node: { id: `inner-${index}`, kind: "primitive", v: index },
        },
      });
    }
  });
  expect(schedule).toHaveBeenCalledTimes(1);
  expect(screen.queryByText("100")).not.toBeInTheDocument();
  act(() => callback(0));
  expect(screen.getByText("100")).toBeInTheDocument();
  await act(async () => finish({ ok: true }));
  expect(screen.getByText("Finished")).toBeInTheDocument();
});

test("Stop keeps results and reports a forced restart clearing the cache", async () => {
  let finish: (value: unknown) => void = () => {};
  invoke.mockImplementation((command, args) => {
    if (command === "run_script") {
      args.onEvent.onmessage({
        method: "stream",
        params: { runId: 1, stream: "stdout", text: "before stop" },
      });
      return new Promise((resolve) => {
        finish = resolve;
      });
    }
    if (command === "stop_script") {
      finish({ ok: false, stopped: true, cacheCleared: true });
    }
    return Promise.resolve();
  });
  render(<App />);
  expect(screen.getByRole("button", { name: "Stop" })).toBeDisabled();
  fireEvent.click(screen.getByRole("button", { name: "▶ Run" }));
  expect(screen.getByRole("button", { name: "Reset" })).toBeDisabled();
  fireEvent.click(screen.getByRole("button", { name: "Stop" }));
  await screen.findByText("Stopped · cache cleared");
  expect(screen.getByText("before stop")).toBeInTheDocument();
  expect(invoke).toHaveBeenCalledWith("stop_script", { scriptId: "scratch-1" });
  expect(screen.getByRole("button", { name: "▶ Run" })).toBeEnabled();
});

test("Reset clears old output and targets only the active script", async () => {
  invoke.mockImplementation(async (command, args) => {
    if (command === "run_script") {
      args.onEvent.onmessage({
        method: "stream",
        params: { runId: 1, stream: "stdout", text: "old output" },
      });
      return { ok: true };
    }
  });
  render(<App />);
  fireEvent.click(screen.getByRole("button", { name: "▶ Run" }));
  await screen.findByText("Finished");
  fireEvent.click(screen.getByRole("button", { name: "Reset" }));
  await screen.findByText("Reset · cache cleared");
  expect(screen.queryByText("old output")).not.toBeInTheDocument();
  expect(invoke).toHaveBeenCalledWith("reset_script", {
    scriptId: "scratch-1",
  });
});

test("a run finishing successfully during Stop still shows Finished", async () => {
  let finish: (value: unknown) => void = () => {};
  invoke.mockImplementation((command) => {
    if (command === "run_script")
      return new Promise((resolve) => {
        finish = resolve;
      });
    if (command === "stop_script")
      finish({ ok: true, stopped: false, cacheCleared: false });
    return Promise.resolve();
  });
  render(<App />);
  fireEvent.click(screen.getByRole("button", { name: "▶ Run" }));
  fireEvent.click(screen.getByRole("button", { name: "Stop" }));
  await screen.findByText("Finished");
  expect(screen.queryByText("Stopped")).not.toBeInTheDocument();
});

test("package preparation diagnostics remain visible when installation fails", async () => {
  invoke.mockImplementation(async (command, args) => {
    if (command !== "run_script") return undefined;
    args.onEvent.onmessage({
      method: "stream",
      params: { runId: 0, stream: "stderr", text: "Preparing Python packages…\nerror: No solution found" },
    });
    throw "Python package preparation failed. See uv output in Results.";
  });
  render(<App />);
  fireEvent.click(screen.getByRole("button", { name: "▶ Run" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("package preparation failed");
  expect(screen.getByText(/No solution found/)).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "▶ Run" })).toBeEnabled();
});

test("opens a library script and preserves its header when saving", async () => {
  const source = "# codingpad: connection=northwind\nprint(1)\n";
  invoke.mockImplementation(async (command, args) => {
    if (command === "load_library_settings")
      return { libraryPath: "/library", pinnedFolders: [] };
    if (command === "list_folder")
      return args.path === "/library"
        ? [{ name: "hello.py", path: "/library/hello.py", isDir: false }]
        : [];
    if (command === "read_script")
      return {
        path: "/library/hello.py",
        name: "hello.py",
        source,
      };
    if (command === "write_script")
      return {
        path: args.path,
        name: "hello.py",
        source: args.source,
      };
    return undefined;
  });
  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: "hello.py" }));
  await waitFor(() =>
    expect(screen.getByLabelText("Code")).toHaveValue(source),
  );
  expect(screen.getByText(/Header: connection=northwind/)).toBeInTheDocument();

  fireEvent.change(screen.getByLabelText("Code"), {
    target: { value: "# codingpad: connection=other\nprint(1)\n" },
  });
  expect(screen.getByText(/Header: connection=other/)).toBeInTheDocument();

  const edited = "# codingpad: connection=other\nprint(2)\n";
  fireEvent.change(screen.getByLabelText("Code"), { target: { value: edited } });
  fireEvent.click(screen.getByRole("button", { name: "Save" }));
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("write_script", {
      path: "/library/hello.py",
      source: edited,
    }),
  );
  expect(screen.getByText("/library/hello.py")).toBeInTheDocument();
});

test("a scratch script saves through the dialog, then saves in place", async () => {
  invoke.mockImplementation(async (command, args) => {
    if (command === "load_library_settings")
      return { libraryPath: "/library", pinnedFolders: [] };
    if (command === "list_folder") return [];
    if (command === "write_script")
      return {
        path: args.path,
        name: "Saved.py",
        source: args.source,
      };
    return undefined;
  });
  dialog.save.mockResolvedValue("/library/Saved.py");
  render(<App />);
  await screen.findByText("Library");
  fireEvent.click(screen.getByRole("button", { name: "Save" }));
  await waitFor(() =>
    expect(dialog.save).toHaveBeenCalledWith(
      expect.objectContaining({ defaultPath: "/library/Scratch 1.py" }),
    ),
  );
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("write_script", {
      path: "/library/Saved.py",
      source: expect.stringContaining("from codingpad import dump"),
    }),
  );
  expect(screen.getByRole("button", { name: "Saved.py" })).toBeInTheDocument();

  fireEvent.change(screen.getByLabelText("Code"), {
    target: { value: "print(2)" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Save" }));
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("write_script", {
      path: "/library/Saved.py",
      source: "print(2)",
    }),
  );
  expect(dialog.save).toHaveBeenCalledTimes(1);
});

test("pinned folders can be added and removed", async () => {
  invoke.mockImplementation(async (command) => {
    if (command === "load_library_settings")
      return { libraryPath: "/library", pinnedFolders: [] };
    if (command === "list_folder") return [];
    return undefined;
  });
  dialog.open.mockResolvedValue("/pinned");
  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: "Pin folder" }));
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("save_library_settings", {
      settings: { libraryPath: "/library", pinnedFolders: ["/pinned"] },
    }),
  );
  fireEvent.click(await screen.findByRole("button", { name: "Unpin pinned" }));
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("save_library_settings", {
      settings: { libraryPath: "/library", pinnedFolders: [] },
    }),
  );
});

test("saving keeps the tab dirty when the editor changes during the write", async () => {
  let finishWrite!: () => void;
  invoke.mockImplementation((command, args) => {
    if (command === "load_library_settings")
      return Promise.resolve({ libraryPath: "/library", pinnedFolders: [] });
    if (command === "list_folder") return Promise.resolve([]);
    if (command === "write_script")
      return new Promise((resolve) => {
        finishWrite = () =>
          resolve({
            path: "/library/x.py",
            name: "x.py",
            source: args.source,
          });
      });
    return Promise.resolve(undefined);
  });
  dialog.save.mockResolvedValue("/library/x.py");
  render(<App />);
  await screen.findByText("Library");
  fireEvent.click(screen.getByRole("button", { name: "Save" }));
  await waitFor(() =>
    expect(
      invoke.mock.calls.some(([command]) => command === "write_script"),
    ).toBe(true),
  );
  fireEvent.change(screen.getByLabelText("Code"), {
    target: { value: "print('new')" },
  });
  await act(async () => finishWrite());
  await waitFor(() =>
    expect(document.querySelector(".dirty-dot")).toBeInTheDocument(),
  );
});

test("closing a dirty tab asks first and keeps it when cancelled", async () => {
  invoke.mockResolvedValue(undefined);
  dialog.ask.mockResolvedValueOnce(false).mockResolvedValue(true);
  render(<App />);
  fireEvent.change(screen.getByLabelText("Code"), {
    target: { value: "a = 1" },
  });
  fireEvent.click(screen.getByRole("button", { name: "New scratch script" }));
  fireEvent.click(screen.getByRole("button", { name: "Close Scratch 1.py" }));
  await waitFor(() =>
    expect(dialog.ask).toHaveBeenCalledWith(
      expect.stringContaining("Discard unsaved changes"),
      expect.objectContaining({ kind: "warning" }),
    ),
  );
  expect(
    screen.getByRole("button", { name: "Scratch 1.py" }),
  ).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Close Scratch 1.py" }));
  await waitFor(() =>
    expect(
      screen.queryByRole("button", { name: "Scratch 1.py" }),
    ).not.toBeInTheDocument(),
  );
});

test("a non-Python file opens but Run is disabled", async () => {
  invoke.mockImplementation(async (command, args) => {
    if (command === "load_library_settings")
      return { libraryPath: "/library", pinnedFolders: [] };
    if (command === "list_folder")
      return [{ name: "notes.rs", path: "/library/notes.rs", isDir: false }];
    if (command === "read_script")
      return {
        path: "/library/notes.rs",
        name: "notes.rs",
        source: "fn main() {}",
      };
    return undefined;
  });
  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: "notes.rs" }));
  await waitFor(() =>
    expect(screen.getByLabelText("Code")).toHaveValue("fn main() {}"),
  );
  expect(screen.getByRole("button", { name: "▶ Run" })).toBeDisabled();
});

test("a rapid double-open only reads the file once", async () => {
  let finishRead!: () => void;
  let reads = 0;
  invoke.mockImplementation((command) => {
    if (command === "load_library_settings")
      return Promise.resolve({ libraryPath: "/library", pinnedFolders: [] });
    if (command === "list_folder")
      return Promise.resolve([
        { name: "x.py", path: "/library/x.py", isDir: false },
      ]);
    if (command === "read_script") {
      reads += 1;
      return new Promise((resolve) => {
        finishRead = () =>
          resolve({
            path: "/library/x.py",
            name: "x.py",
            source: "print(1)",
          });
      });
    }
    return Promise.resolve(undefined);
  });
  render(<App />);
  const file = await screen.findByRole("button", { name: "x.py" });
  fireEvent.click(file);
  fireEvent.click(file);
  await act(async () => finishRead());
  expect(reads).toBe(1);
  await waitFor(() => expect(document.querySelectorAll(".tab")).toHaveLength(2));
});

test("changing the library removes that path from pinned folders", async () => {
  invoke.mockImplementation(async (command) => {
    if (command === "load_library_settings")
      return { libraryPath: "/library", pinnedFolders: ["/pinned"] };
    if (command === "list_folder") return [];
    return undefined;
  });
  dialog.open.mockResolvedValue("/pinned");
  render(<App />);
  fireEvent.click(
    await screen.findByRole("button", { name: "Change library folder" }),
  );
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("save_library_settings", {
      settings: { libraryPath: "/pinned", pinnedFolders: [] },
    }),
  );
});

test("saving refreshes the tree without collapsing an expanded folder", async () => {
  invoke.mockImplementation(async (command, args) => {
    if (command === "load_library_settings")
      return { libraryPath: "/library", pinnedFolders: [] };
    if (command === "list_folder")
      return args.path === "/library"
        ? [{ name: "sub", path: "/library/sub", isDir: true }]
        : [{ name: "x.py", path: "/library/sub/x.py", isDir: false }];
    if (command === "read_script")
      return {
        path: "/library/sub/x.py",
        name: "x.py",
        source: "print(1)",
      };
    if (command === "write_script")
      return {
        path: args.path,
        name: "x.py",
        source: args.source,
      };
    return undefined;
  });
  render(<App />);
  const sub = await screen.findByRole("button", { name: /sub/ });
  fireEvent.click(sub);
  expect(sub).toHaveAttribute("aria-expanded", "true");
  fireEvent.click(await screen.findByRole("button", { name: "x.py" }));
  await waitFor(() =>
    expect(screen.getByLabelText("Code")).toHaveValue("print(1)"),
  );
  fireEvent.change(screen.getByLabelText("Code"), {
    target: { value: "print(2)" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Save" }));
  await waitFor(() =>
    expect(
      invoke.mock.calls.some(([command]) => command === "write_script"),
    ).toBe(true),
  );
  expect(screen.getByRole("button", { name: /sub/ })).toHaveAttribute(
    "aria-expanded",
    "true",
  );
});

test("a failed refresh shows an error but keeps the last tree", async () => {
  let listings = 0;
  invoke.mockImplementation(async (command, args) => {
    if (command === "load_library_settings")
      return { libraryPath: "/library", pinnedFolders: [] };
    if (command === "list_folder") {
      listings += 1;
      if (listings > 1) throw new Error("folder is gone");
      return [{ name: "x.py", path: "/library/x.py", isDir: false }];
    }
    if (command === "read_script")
      return {
        path: "/library/x.py",
        name: "x.py",
        source: "print(1)",
      };
    if (command === "write_script")
      return {
        path: args.path,
        name: "x.py",
        source: args.source,
      };
    return undefined;
  });
  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: "x.py" }));
  await waitFor(() =>
    expect(screen.getByLabelText("Code")).toHaveValue("print(1)"),
  );
  fireEvent.change(screen.getByLabelText("Code"), {
    target: { value: "print(2)" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Save" }));
  expect(await screen.findByText(/folder is gone/)).toBeInTheDocument();
  expect(screen.getAllByRole("button", { name: "x.py" })).toHaveLength(2);
});

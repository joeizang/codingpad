import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import App from "./App";
import { resetPendingCrashForTests } from "./crash";
import type { KernelEvent } from "./protocol";
import { resetUpdateCheckForTests } from "./updates";

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
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));
vi.mock("./CodeEditor", () => ({
  default: ({
    source,
    onChange,
    onDiagnostics,
    toolchainVersion,
    compileDiagnostics,
  }: {
    toolchainVersion?: number;
    compileDiagnostics?: { message: string }[];
    source: string;
    onChange: (source: string) => void;
    onDiagnostics?: (summary: {
      errors: number;
      warnings: number;
      message?: string;
    }) => void;
  }) => (
    <div>
      <span data-testid="compiler-diagnostics">{compileDiagnostics?.map((d) => d.message).join(";")}</span>
      <textarea
        aria-label="Code"
        data-toolchain-version={toolchainVersion}
        value={source}
        onChange={(e) => onChange(e.target.value)}
      />
      {onDiagnostics && (
        <button
          aria-label="Diagnostics"
          onClick={() => onDiagnostics({ errors: 1, warnings: 2 })}
        />
      )}
    </div>
  ),
}));
beforeEach(() => {
  invoke.mockReset();
  resetUpdateCheckForTests();
  resetPendingCrashForTests();
  dialog.open.mockReset();
  dialog.save.mockReset();
  dialog.ask.mockReset();
  dialog.ask.mockResolvedValue(true);
  localStorage.clear();
});

/** Opens the new-script panel and creates a script in `language`. */
function createScript(language: "Python" | "C#" | "TypeScript" | "Rust" | "SQL" = "Python") {
  fireEvent.click(screen.getByRole("button", { name: "New script" }));
  fireEvent.click(screen.getByRole("radio", { name: language }));
  fireEvent.click(screen.getByRole("button", { name: /^Create / }));
}

test("a newer release shows a notice linking to its page", async () => {
  invoke.mockImplementation(async (command) => {
    if (command === "check_for_update")
      return {
        version: "0.2.0",
        url: "https://github.com/joeizang/codingpad/releases/tag/v0.2.0",
      };
    return undefined;
  });
  render(<App />);
  expect(
    await screen.findByText("CodingPad 0.2.0 is available."),
  ).toBeInTheDocument();
  expect(screen.getByRole("link", { name: "View release" })).toHaveAttribute(
    "href",
    "https://github.com/joeizang/codingpad/releases/tag/v0.2.0",
  );
});

test("a current build shows no notice", async () => {
  invoke.mockImplementation(async (command) => {
    if (command === "check_for_update") return null;
    return undefined;
  });
  render(<App />);
  await waitFor(() =>
    expect(
      invoke.mock.calls.filter(([command]) => command === "check_for_update"),
    ).toHaveLength(1),
  );
  expect(screen.queryByText(/is available\./)).not.toBeInTheDocument();
});

test("a failed update check stays silent and leaves the app working", async () => {
  invoke.mockImplementation(async (command) => {
    if (command === "check_for_update") throw new Error("offline");
    return undefined;
  });
  render(<App />);
  expect(screen.getByRole("button", { name: "▶ Run" })).toBeInTheDocument();
  await waitFor(() =>
    expect(
      invoke.mock.calls.filter(([command]) => command === "check_for_update"),
    ).toHaveLength(1),
  );
  expect(screen.queryByText(/is available\./)).not.toBeInTheDocument();
  expect(screen.queryByRole("alert")).not.toBeInTheDocument();
});

test("type diagnostics are shown without disabling Run", async () => {
  invoke.mockResolvedValue({ ok: true });
  render(<App />);
  fireEvent.click(screen.getByRole("button", { name: "Diagnostics" }));
  expect(screen.getByText("1 error · 2 warnings")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "▶ Run" })).toBeEnabled();
  fireEvent.click(screen.getByRole("button", { name: "▶ Run" }));
  expect(
    invoke.mock.calls.filter(([command]) => command === "run_script"),
  ).toHaveLength(1);
});

test("Run renders streamed text, structured dumps and completion", async () => {
  invoke.mockImplementation(async (command, args) => {
    if (command !== "run_script") return undefined;
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
      scriptId: expect.stringMatching(/^scratch-/),
      source: expect.stringContaining("from codingpad import dump"),
    }),
  );
});

test("startup failure is visible and Run becomes available again", async () => {
  invoke.mockImplementation(async (command: string) => {
    if (command === "run_script")
      throw "uv was not found. Install uv and restart CodingPad.";
    return undefined;
  });
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
  createScript();
  expect(screen.getByLabelText("Code")).not.toHaveValue("a = 1");
  fireEvent.click(screen.getByRole("button", { name: "Scratch 1.py" }));
  expect(screen.getByLabelText("Code")).toHaveValue("a = 1");
  fireEvent.click(screen.getByRole("button", { name: "Close Scratch 1.py" }));
  await waitFor(() =>
    expect(
      screen.queryByRole("button", { name: "Scratch 1.py" }),
    ).not.toBeInTheDocument(),
  );
  expect(invoke).toHaveBeenCalledWith(
    "close_script",
    expect.objectContaining({
      scriptId: expect.stringMatching(/^scratch-/),
    }),
  );
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
    if (
      command === "load_library_settings" ||
      command === "list_samples" ||
      command === "check_for_update" ||
      command === "list_connections" ||
      command === "take_pending_crash"
    )
      return Promise.resolve(undefined);
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
  invoke.mockImplementation(async (command, args) => {
    if (command !== "run_script") return undefined;
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
    if (command !== "run_script") return undefined;
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
  invoke.mockImplementation(async (command, args) => {
    if (command !== "run_script") return undefined;
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
    if (
      command === "load_library_settings" ||
      command === "list_samples" ||
      command === "check_for_update" ||
      command === "list_connections" ||
      command === "take_pending_crash"
    )
      return Promise.resolve(undefined);
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
  expect(invoke).toHaveBeenCalledWith(
    "stop_script",
    expect.objectContaining({
      scriptId: expect.stringMatching(/^scratch-/),
    }),
  );
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
  expect(invoke).toHaveBeenCalledWith(
    "reset_script",
    expect.objectContaining({
      scriptId: expect.stringMatching(/^scratch-/),
    }),
  );
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
    if (command === "run_script") return { ok: true };
    return undefined;
  });
  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: "hello.py" }));
  await waitFor(() =>
    expect(screen.getByLabelText("Code")).toHaveValue(source),
  );
  expect(screen.getByText("northwind", { selector: ".context-item b" })).toBeInTheDocument();

  fireEvent.change(screen.getByLabelText("Code"), {
    target: { value: "# codingpad: connection=other\nprint(1)\n" },
  });
  expect(screen.getByText("other", { selector: ".context-item b" })).toBeInTheDocument();

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
  fireEvent.click(screen.getByRole("button", { name: "▶ Run" }));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith(
    "run_script", expect.objectContaining({ path: "/library/hello.py", source: edited }),
  ));
  await screen.findByText("Finished");
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
  createScript();
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

test.each([
  { file: "notes.txt", runnable: false },
  { file: "script.rs", runnable: true },
])("$file opens with the expected Run availability", async ({ file, runnable }) => {
  invoke.mockImplementation(async (command, args) => {
    if (command === "load_library_settings")
      return { libraryPath: "/library", pinnedFolders: [] };
    if (command === "list_folder")
      return [{ name: file, path: `/library/${file}`, isDir: false }];
    if (command === "read_script")
      return {
        path: `/library/${file}`,
        name: file,
        source: "fn main() {}",
      };
    if (command === "run_script") return { ok: true };
    return undefined;
  });
  render(<App />);
  fireEvent.click(await screen.findByRole("button", { name: file }));
  await waitFor(() =>
    expect(screen.getByLabelText("Code")).toHaveValue("fn main() {}"),
  );
  const run = screen.getByRole("button", { name: "▶ Run" });
  if (!runnable) {
    expect(run).toBeDisabled();
    return;
  }
  fireEvent.click(run);
  await screen.findByText("Finished");
  expect(invoke).toHaveBeenCalledWith("run_script", expect.objectContaining({
    language: "rust", path: "/library/script.rs", source: "fn main() {}",
  }));
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

test("samples open as unsaved copies and keep their original source", async () => {
  invoke.mockImplementation(async (command: string) => {
    if (command === "load_library_settings")
      return { libraryPath: "/library", pinnedFolders: [] };
    if (command === "list_folder") return [];
    if (command === "list_samples")
      return [
        {
          id: "python/hello",
          name: "hello.py",
          title: "Hello, world!",
          language: "python",
          source: "print('from the sample')",
        },
        {
          id: "typescript/hello",
          name: "hello.ts",
          title: "Hello, TypeScript!",
          language: "typescript",
          source: "console.log('from the TypeScript sample')",
        },
      ];
    if (command === "run_script") return { ok: true };
    return undefined;
  });
  render(<App />);
  fireEvent.click(
    await screen.findByRole("button", { name: /^Hello, world!/ }),
  );
  expect(screen.getByLabelText("Code")).toHaveValue("print('from the sample')");
  expect(screen.getByText(/Sample copy/)).toBeInTheDocument();
  fireEvent.change(screen.getByLabelText("Code"), {
    target: { value: "print('my copy')" },
  });
  // Reopening the sample focuses the existing copy instead of discarding edits.
  fireEvent.click(screen.getByRole("button", { name: /^Hello, world!/ }));
  expect(screen.getByLabelText("Code")).toHaveValue("print('my copy')");

  fireEvent.click(screen.getByRole("button", { name: /TypeScript.*1/ }));
  fireEvent.click(screen.getByRole("button", { name: /^Hello, TypeScript!/ }));
  expect(screen.getByLabelText("Code")).toHaveValue(
    "console.log('from the TypeScript sample')",
  );
  fireEvent.click(screen.getByRole("button", { name: "▶ Run" }));
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith(
      "run_script",
      expect.objectContaining({
        language: "typescript",
        source: "console.log('from the TypeScript sample')",
      }),
    ),
  );
});

test("a failed sample list is reported in the Samples section", async () => {
  invoke.mockImplementation(async (command: string) => {
    if (command === "load_library_settings")
      return { libraryPath: "/library", pinnedFolders: [] };
    if (command === "list_folder") return [];
    if (command === "list_samples") throw "Could not list samples";
    return undefined;
  });
  render(<App />);
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Could not list samples",
  );
  expect(screen.getByRole("heading", { name: "Samples" })).toBeInTheDocument();
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

test("missing uv offers one-click installation and reruns the latest source", async () => {
  let installed = false;
  let finish!: () => void;
  invoke.mockImplementation(async (command, args) => {
    if (command === "run_script") {
      if (!installed) throw "uv was not found. Install a private copy.";
      return { ok: true };
    }
    if (command === "install_uv") {
      args.onProgress.onmessage("Downloading and verifying uv…");
      await new Promise<void>((resolve) => { finish = resolve; });
      installed = true;
    }
  });
  render(<App />);
  fireEvent.click(screen.getByRole("button", { name: "▶ Run" }));
  fireEvent.click(await screen.findByRole("button", { name: "Install uv and run" }));
  expect(screen.getByRole("button", { name: "Installing uv…" })).toBeDisabled();
  expect(screen.getByRole("button", { name: "▶ Run" })).toBeDisabled();
  fireEvent.change(screen.getByLabelText("Code"), { target: { value: "42" } });
  await act(async () => finish());
  await screen.findByText("Finished");
  expect(screen.getByLabelText("Code")).toHaveAttribute("data-toolchain-version", "1");
  expect(invoke).toHaveBeenLastCalledWith("run_script", expect.objectContaining({ source: "42" }));
  expect(screen.queryByRole("button", { name: "Install uv and run" })).not.toBeInTheDocument();
});

test("missing .NET offers one-click SDK installation and reruns the latest source", async () => {
  let installed = false;
  let finish!: () => void;
  invoke.mockImplementation(async (command, args) => {
    if (command === "run_script") {
      if (!installed) throw ".NET was not found. Install the .NET SDK and run again.";
      return { ok: true };
    }
    if (command === "install_dotnet") {
      args.onProgress.onmessage("Downloading and verifying the .NET install script…");
      await new Promise<void>((resolve) => { finish = resolve; });
      installed = true;
    }
  });
  render(<App />);
  createScript("C#");
  fireEvent.click(screen.getByRole("button", { name: "▶ Run" }));
  fireEvent.click(await screen.findByRole("button", { name: "Install .NET and run" }));
  expect(screen.getByRole("button", { name: "Installing .NET…" })).toBeDisabled();
  expect(screen.getByRole("button", { name: "▶ Run" })).toBeDisabled();
  fireEvent.change(screen.getByLabelText("Code"), { target: { value: "42.Dump();" } });
  await act(async () => finish());
  await screen.findByText("Finished");
  expect(screen.getByLabelText("Code")).toHaveAttribute("data-toolchain-version", "1");
  expect(invoke).toHaveBeenLastCalledWith("run_script", expect.objectContaining({ source: "42.Dump();" }));
  expect(screen.queryByRole("button", { name: "Install .NET and run" })).not.toBeInTheDocument();
});

test("missing Bun offers one-click installation and reruns the latest source", async () => {
  let installed = false;
  let finish!: () => void;
  invoke.mockImplementation(async (command, args) => {
    if (command === "run_script") {
      if (!installed) throw "Bun was not found. Choose Install Bun and run.";
      return { ok: true };
    }
    if (command === "install_bun") {
      args.onProgress.onmessage("Downloading and verifying Bun…");
      await new Promise<void>((resolve) => { finish = resolve; });
      installed = true;
    }
  });
  render(<App />);
  createScript("TypeScript");
  fireEvent.click(screen.getByRole("button", { name: "▶ Run" }));
  fireEvent.click(await screen.findByRole("button", { name: "Install Bun and run" }));
  expect(screen.getByRole("button", { name: "Installing Bun…" })).toBeDisabled();
  expect(screen.getByRole("button", { name: "▶ Run" })).toBeDisabled();
  fireEvent.change(screen.getByLabelText("Code"), { target: { value: 'console.log("hi");' } });
  await act(async () => finish());
  await screen.findByText("Finished");
  expect(screen.getByLabelText("Code")).toHaveAttribute("data-toolchain-version", "1");
  expect(invoke).toHaveBeenLastCalledWith("run_script", expect.objectContaining({ source: 'console.log("hi");' }));
  expect(screen.queryByRole("button", { name: "Install Bun and run" })).not.toBeInTheDocument();
});

test("missing Rust offers one-click installation and reruns the latest source", async () => {
  let installed = false;
  let finish!: () => void;
  invoke.mockImplementation(async (command, args) => {
    if (command === "run_script") {
      if (!installed) throw "Rust was not found. Choose Install Rust and run.";
      return { ok: true };
    }
    if (command === "install_rust") {
      args.onProgress.onmessage("Downloading and verifying Rust…");
      await new Promise<void>((resolve) => { finish = resolve; });
      installed = true;
    }
  });
  render(<App />);
  createScript("Rust");
  fireEvent.click(screen.getByRole("button", { name: "▶ Run" }));
  fireEvent.click(await screen.findByRole("button", { name: "Install Rust and run" }));
  expect(screen.getByRole("button", { name: "Installing Rust…" })).toBeDisabled();
  expect(screen.getByRole("button", { name: "▶ Run" })).toBeDisabled();
  fireEvent.change(screen.getByLabelText("Code"), { target: { value: 'println!("hi");' } });
  await act(async () => finish());
  await screen.findByText("Finished");
  expect(screen.getByLabelText("Code")).toHaveAttribute("data-toolchain-version", "1");
  expect(invoke).toHaveBeenLastCalledWith("run_script", expect.objectContaining({ source: 'println!("hi");' }));
  expect(screen.queryByRole("button", { name: "Install Rust and run" })).not.toBeInTheDocument();
});

test("failed Bun installation can be retried without rerunning prematurely", async () => {
  invoke.mockImplementation(async (command) => {
    if (command === "run_script") throw "Bun was not found. Choose Install Bun and run.";
    if (command === "install_bun") throw "Tool download failed SHA-256 verification.";
  });
  render(<App />);
  createScript("TypeScript");
  fireEvent.click(screen.getByRole("button", { name: "▶ Run" }));
  fireEvent.click(await screen.findByRole("button", { name: "Install Bun and run" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("SHA-256 verification");
  expect(screen.getByLabelText("Code")).toHaveAttribute("data-toolchain-version", "0");
  expect(screen.getByRole("button", { name: "Install Bun and run" })).toBeEnabled();
  expect(invoke.mock.calls.filter(([command]) => command === "run_script")).toHaveLength(1);
});

test("failed uv installation can be retried without rerunning prematurely", async () => {
  invoke.mockImplementation(async (command) => {
    if (command === "run_script") throw "uv was not found. Install a private copy.";
    if (command === "install_uv") throw "Tool download failed SHA-256 verification.";
  });
  render(<App />);
  fireEvent.click(screen.getByRole("button", { name: "▶ Run" }));
  fireEvent.click(await screen.findByRole("button", { name: "Install uv and run" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("SHA-256 verification");
  expect(screen.getByLabelText("Code")).toHaveAttribute("data-toolchain-version", "0");
  expect(screen.getByRole("button", { name: "Install uv and run" })).toBeEnabled();
  expect(invoke.mock.calls.filter(([command]) => command === "run_script")).toHaveLength(1);
});

test("ordinary script errors never offer a uv installation", async () => {
  invoke.mockImplementation(async (command) => {
    if (command === "run_script") throw "Could not start Python: permission denied";
  });
  render(<App />);
  fireEvent.click(screen.getByRole("button", { name: "▶ Run" }));
  await screen.findByRole("alert");
  expect(screen.queryByRole("button", { name: "Install uv and run" })).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Install .NET and run" })).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Install Bun and run" })).not.toBeInTheDocument();
});


test("C# scratch runs with compiler diagnostics and clears stale diagnostics on edits", async () => {
  invoke.mockImplementation(async (command, args) => {
    if (command !== "run_script") return;
    args.onEvent.onmessage({ method: "diagnostics", params: { runId: 1, diagnostics: [{ message: "CS0103: missing name", line: 1, column: 1, endLine: 1, endColumn: 8 }] } });
    return { ok: false };
  });
  render(<App />);
  createScript("C#");
  expect(screen.getByRole("region", { name: "C# editor" })).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "▶ Run" }));
  await screen.findByText("Script error");
  expect(invoke).toHaveBeenCalledWith("run_script", expect.objectContaining({ language: "csharp", source: expect.stringContaining("Console.WriteLine") }));
  expect(screen.getByTestId("compiler-diagnostics")).toHaveTextContent("CS0103");
  fireEvent.change(screen.getByLabelText("Code"), { target: { value: "42.Dump();" } });
  expect(screen.getByTestId("compiler-diagnostics")).toBeEmptyDOMElement();
});

test("a TypeScript scratch runs through the kernel and shows a missing Bun error on retry", async () => {
  let hasBun = true;
  invoke.mockImplementation(async (command, args) => {
    if (command !== "run_script") return;
    if (!hasBun) throw "Bun was not found. Install Bun and try again.";
    args.onEvent.onmessage({ method: "stream", params: { runId: 1, stream: "stdout", text: "Hello from TypeScript!" } });
    args.onEvent.onmessage({ method: "dump", params: { runId: 1, node: { id: "1:n1", kind: "primitive", v: 42 } } });
    return { ok: true };
  });
  render(<App />);
  createScript("TypeScript");
  expect(screen.getByRole("region", { name: "TypeScript editor" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: /^Scratch \d+\.ts$/ })).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "▶ Run" }));
  await screen.findByText("Hello from TypeScript!");
  expect(screen.getByText("42")).toBeInTheDocument();
  expect(invoke).toHaveBeenCalledWith("run_script", expect.objectContaining({
    language: "typescript", source: expect.stringContaining('from "codingpad"'),
  }));
  hasBun = false;
  fireEvent.click(screen.getByRole("button", { name: "▶ Run" }));
  await screen.findByText(/Bun was not found/);
  expect(
    screen.getByRole("button", { name: "Install Bun and run" }),
  ).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "▶ Run" })).toBeEnabled();
});

test("SQL tabs run through the direct connection and render table results", async () => {
  invoke.mockImplementation(async (command: string) => {
    if (command === "connection_schema") return { kind: "sqlite", tables: [{ schema: "main", name: "sample_table", columns: [] }] };
    if (command === "run_sql") {
      return {
        ok: true,
        stopped: false,
        tables: [
          {
            id: "sql-1:0",
            kind: "table",
            type: "query",
            columns: [{ name: "answer" }],
            rows: [[{ id: "sql:r0c0", kind: "primitive", v: 42 }]],
            total: 1,
            next: null,
            handle: "sql-1",
          },
        ],
      };
    }
    return undefined;
  });
  render(<App />);
  createScript("SQL");
  fireEvent.click(screen.getByRole("button", { name: "▶ Run" }));
  await waitFor(() =>
    expect(screen.getByRole("button", { name: "▶ Run" })).toBeEnabled(),
  );
  expect(
    invoke.mock.calls.filter(([command]) => command === "run_sql"),
  ).toHaveLength(1);
  expect(invoke).toHaveBeenCalledWith("connection_schema", { name: "codingpad-sample" });
  expect(await screen.findByRole("button", { name: /main.sample_table/ })).toBeInTheDocument();
  expect(screen.getByText("42")).toBeInTheDocument();
  expect(screen.getByText("Finished")).toBeInTheDocument();
});

test("exporting a SQL result asks for a file and exports through the host", async () => {
  invoke.mockImplementation(async (command: string, args: { onProgress?: { onmessage: (p: unknown) => void } }) => {
    if (command === "connection_schema") return { kind: "sqlite", tables: [] };
    if (command === "run_sql") {
      return {
        ok: true,
        stopped: false,
        tables: [
          {
            id: "sql-1:0",
            kind: "table",
            type: "query",
            columns: [{ name: "answer" }],
            rows: [[{ id: "sql:r0c0", kind: "primitive", v: 42 }]],
            total: 1,
            next: null,
            handle: "sql-1",
          },
        ],
      };
    }
    if (command === "export_table_dump") {
      args.onProgress?.onmessage({ rows: 1, total: 1 });
      return { rows: 1, shortenedCells: 0 };
    }
    return undefined;
  });
  dialog.save.mockResolvedValueOnce("/tmp/answers");
  render(<App />);
  createScript("SQL");
  fireEvent.click(screen.getByRole("button", { name: "▶ Run" }));
  fireEvent.click(await screen.findByRole("button", { name: "Export CSV" }));
  expect(await screen.findByText("Exported 1 row")).toBeInTheDocument();
  expect(dialog.save).toHaveBeenCalledWith(
    expect.objectContaining({ filters: [{ name: "CSV", extensions: ["csv"] }] }),
  );
  expect(invoke).toHaveBeenCalledWith(
    "export_table_dump",
    expect.objectContaining({ handle: "sql-1", sql: true, format: "csv", path: "/tmp/answers.csv" }),
  );
});

test("a new script attached to a connection starts with its header line", async () => {
  invoke.mockImplementation(async (command: string) =>
    command === "list_connections" ? [{ id: "1", name: "shop", kind: "postgres" }] : undefined,
  );
  render(<App />);
  fireEvent.click(screen.getByRole("button", { name: "New script" }));
  fireEvent.keyDown(screen.getByRole("dialog", { name: "New script" }), { key: "c" });
  fireEvent.click(await screen.findByRole("button", { name: /^shop/ }));
  fireEvent.click(screen.getByRole("button", { name: "Create C# script on shop" }));
  expect(screen.queryByRole("dialog", { name: "New script" })).not.toBeInTheDocument();
  expect((screen.getByLabelText("Code") as HTMLTextAreaElement).value).toMatch(
    /^\/\/ codingpad: connection=shop\n\nConsole.WriteLine/,
  );
  expect(screen.getByText("shop", { selector: ".context-item b" })).toBeInTheDocument();
  expect(invoke).toHaveBeenCalledWith("connection_schema", { name: "shop" });
});

test("Ctrl+N opens the new-script panel", () => {
  invoke.mockResolvedValue(undefined);
  render(<App />);
  fireEvent.keyDown(window, { key: "n", ctrlKey: true });
  expect(screen.getByRole("dialog", { name: "New script" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "New script" })).toHaveAttribute("aria-expanded", "true");
});

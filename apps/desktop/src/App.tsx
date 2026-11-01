import { useEffect, useRef, useState } from "react";
import { Channel, invoke } from "@tauri-apps/api/core";
import CodeEditor, { type EditorHandle } from "./CodeEditor";
import { DumpTree } from "./DumpTree";
import {
  appendOutputBatch,
  collectDumpUpdates,
  type DumpNode,
  type KernelEvent,
  type Output,
} from "./protocol";

const SAMPLE = `from codingpad import dump

print("Hello from CodingPad!")

dump({"name": "Ada", "languages": ["Python", "C#", "Rust"]})

# The last expression is displayed automatically.
6 * 7
`;

type Script = {
  id: string;
  name: string;
  source: string;
  output: Output[];
  running: boolean;
  status: string;
  version: number;
  updates: Record<string, DumpNode>;
  controlling?: boolean;
  error?: string;
};
const newScript = (number: number): Script => ({
  id: `scratch-${number}`,
  name: `Scratch ${number}.py`,
  source: SAMPLE,
  output: [],
  running: false,
  status: "Ready",
  version: 0,
  updates: {},
});

export default function App() {
  const [scripts, setScripts] = useState<Script[]>([newScript(1)]);
  const [activeId, setActiveId] = useState("scratch-1");
  const counter = useRef(1);
  const [closing, setClosing] = useState(false);
  const generation = useRef(new Map<string, number>());
  const running = useRef(new Set<string>());
  const editor = useRef<EditorHandle | null>(null);
  const frames = useRef(new Map<string, number>());
  const cancelFrame = (id: string) => {
    const frame = frames.current.get(id);
    if (frame !== undefined) cancelAnimationFrame(frame);
    frames.current.delete(id);
  };
  useEffect(
    () => () => {
      for (const frame of frames.current.values()) cancelAnimationFrame(frame);
      frames.current.clear();
      generation.current.clear();
      running.current.clear();
    },
    [],
  );
  const active = scripts.find((script) => script.id === activeId)!;
  const update = (id: string, change: (script: Script) => Script) =>
    setScripts((all) =>
      all.map((script) => (script.id === id ? change(script) : script)),
    );

  async function runScript(script: Script) {
    if (running.current.has(script.id) || script.controlling) return;
    running.current.add(script.id);
    const version = (generation.current.get(script.id) ?? 0) + 1;
    generation.current.set(script.id, version);
    cancelFrame(script.id);
    update(script.id, (item) => ({
      ...item,
      output: [],
      running: true,
      status: "Running…",
      version,
      updates: {},
      error: undefined,
    }));
    const pending: KernelEvent[] = [];
    const flush = () => {
      cancelFrame(script.id);
      const batch = pending.splice(0);
      if (generation.current.get(script.id) === version && batch.length) {
        const updates = collectDumpUpdates(batch);
        update(script.id, (item) => ({
          ...item,
          output: appendOutputBatch(item.output, batch),
          updates: Object.keys(updates).length
            ? { ...item.updates, ...updates }
            : item.updates,
        }));
      }
    };
    const onEvent = new Channel<KernelEvent>();
    onEvent.onmessage = (event) => {
      // A channel can deliver late, even after its invocation resolved. Keep the
      // generation check so an earlier run cannot append to a newer run's output.
      if (generation.current.get(script.id) !== version) return;
      if (event.method === "run_finished") return;
      pending.push(event);
      if (!frames.current.has(script.id)) {
        frames.current.set(script.id, requestAnimationFrame(flush));
      }
    };
    try {
      const result = await invoke<{
        ok: boolean;
        stopped?: boolean;
        cacheCleared?: boolean;
      }>("run_script", {
        scriptId: script.id,
        source: script.source,
        onEvent,
      });
      update(script.id, (item) => ({
        ...item,
        status: result.cacheCleared
          ? "Stopped · kernel restarted; cache cleared"
          : result.stopped
            ? "Stopped"
            : result.ok
              ? "Finished"
              : "Script error",
      }));
    } catch (error) {
      update(script.id, (item) => ({
        ...item,
        status: "Could not run",
        error: String(error),
      }));
    } finally {
      flush();
      running.current.delete(script.id);
      update(script.id, (item) => ({ ...item, running: false }));
    }
  }

  async function controlScript(script: Script, action: "stop" | "reset") {
    if (script.controlling) return;
    update(script.id, (item) => ({
      ...item,
      controlling: true,
      error: undefined,
      status: action === "stop" ? "Stopping…" : "Resetting…",
    }));
    try {
      await invoke(`${action}_script`, { scriptId: script.id });
      if (action === "stop") {
        update(script.id, (item) => ({
          ...item,
          status:
            !item.running && item.status === "Stopping…"
              ? "Stopped"
              : item.status,
        }));
      }
      if (action === "reset") {
        cancelFrame(script.id);
        generation.current.set(
          script.id,
          (generation.current.get(script.id) ?? 0) + 1,
        );
        update(script.id, (item) => ({
          ...item,
          output: [],
          updates: {},
          version: item.version + 1,
          status: "Reset · cache cleared",
        }));
      }
    } catch (error) {
      update(script.id, (item) => ({
        ...item,
        status: "Could not " + action,
        error: String(error),
      }));
    } finally {
      update(script.id, (item) => ({ ...item, controlling: false }));
    }
  }

  async function expandNode(
    handle: string,
    cursor: string | null,
  ): Promise<DumpNode> {
    const result = await invoke<{ node: DumpNode }>("expand_node", {
      scriptId: active.id,
      handle,
      cursor,
      limit: 100,
    });
    return result.node;
  }

  function openLine(line: number) {
    const current = editor.current;
    if (!current) return;
    current.revealLineInCenter(line);
    current.setPosition({ lineNumber: line, column: 1 });
    current.focus();
  }

  function addScript() {
    const script = newScript(++counter.current);
    setScripts((all) => [...all, script]);
    setActiveId(script.id);
  }

  async function closeScript(script: Script) {
    if (script.running || script.controlling || closing || scripts.length === 1)
      return;
    setClosing(true);
    try {
      await invoke("close_script", { scriptId: script.id });
      cancelFrame(script.id);
      generation.current.delete(script.id);
      running.current.delete(script.id);
      setScripts((all) => all.filter((item) => item.id !== script.id));
      setActiveId((current) =>
        current === script.id
          ? scripts.find((item) => item.id !== script.id)!.id
          : current,
      );
    } catch (error) {
      update(script.id, (item) => ({ ...item, error: String(error) }));
    } finally {
      setClosing(false);
    }
  }

  // The editor command reads current state instead of its mount-time closure.
  const runActive = useRef(() => {});
  runActive.current = () => {
    void runScript(active);
  };

  return (
    <main className="app">
      <header className="toolbar">
        <div className="brand">
          <span className="brand-mark">c.</span>
          <strong>CodingPad</strong>
          <span className="language">Python</span>
        </div>
        <div className="run-controls">
          <button
            className="run-button"
            disabled={active.running || active.controlling}
            onClick={() => void runScript(active)}
          >
            {active.running ? "Running…" : "▶ Run"}
          </button>
          <button
            className="control-button"
            disabled={!active.running || active.controlling}
            onClick={() => void controlScript(active, "stop")}
          >
            Stop
          </button>
          <button
            className="control-button"
            disabled={active.running || active.controlling}
            onClick={() => void controlScript(active, "reset")}
          >
            Reset
          </button>
        </div>
      </header>
      <nav className="tabs" aria-label="Scratch scripts">
        {scripts.map((script) => (
          <div
            className={`tab ${script.id === activeId ? "active" : ""}`}
            key={script.id}
          >
            <button
              aria-current={script.id === activeId ? "page" : undefined}
              onClick={() => setActiveId(script.id)}
            >
              {script.name}
              {script.running ? " · running" : ""}
            </button>
            {scripts.length > 1 && (
              <button
                aria-label={`Close ${script.name}`}
                disabled={script.running || script.controlling || closing}
                onClick={() => void closeScript(script)}
              >
                ×
              </button>
            )}
          </div>
        ))}
        <button
          className="add-tab"
          aria-label="New scratch script"
          onClick={addScript}
        >
          +
        </button>
      </nav>
      <div className="workspace">
        <section className="editor-pane" aria-label="Python editor">
          <div className="pane-heading">
            <span>EDITOR</span>
            <span>Ctrl / ⌘ + Enter to run</span>
          </div>
          <div className="editor">
            <CodeEditor
              key={active.id}
              id={active.id}
              source={active.source}
              onChange={(source) =>
                update(active.id, (script) => ({ ...script, source }))
              }
              onRun={() => runActive.current()}
              onMount={(instance) => {
                editor.current = instance;
              }}
            />
          </div>
        </section>
        <section className="results-pane" aria-label="Results">
          <div className="pane-heading">
            <span>RESULTS</span>
            <span role="status">{active.status}</span>
          </div>
          <div className="results" aria-busy={active.running}>
            {active.error && (
              <div className="error" role="alert">
                {active.error}
              </div>
            )}
            {active.output.length === 0 && !active.error && (
              <div className="empty">
                {active.running
                  ? "Waiting for output…"
                  : "Run your script to see printed output and values here."}
              </div>
            )}
            {active.output.map((output) =>
              output.kind === "stream" ? (
                <pre
                  className={`stream ${output.stream}`}
                  key={`${active.id}:${active.version}:${output.id}`}
                >
                  {output.text}
                </pre>
              ) : (
                <div
                  className="dump"
                  key={`${active.id}:${active.version}:${output.id}`}
                >
                  <DumpTree
                    node={output.node}
                    updates={active.updates}
                    expand={expandNode}
                    onOpenLine={openLine}
                  />
                </div>
              ),
            )}
          </div>
        </section>
      </div>
      <footer>
        <span>{active.name} · Scratch scripts are unsaved</span>
        <span>Python · Run control</span>
      </footer>
    </main>
  );
}

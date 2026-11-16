import { useEffect, useRef, useState } from "react";
import { Channel, invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  ask,
  open as openDialog,
  save as saveDialog,
} from "@tauri-apps/plugin-dialog";
import CodeEditor, { type EditorHandle } from "./CodeEditor";
import { ConnectionsDialog } from "./ConnectionsDialog";
import {
  DumpTree,
  type ExportFormat,
  type ExportSummary,
} from "./DumpTree";
import { uniqueId, type DiagnosticsSummary } from "./lsp";
import {
  appendOutputBatch,
  collectDumpUpdates,
  type CompileDiagnostic,
  type DumpNode,
  type KernelEvent,
  type Output,
} from "./protocol";
import { SecretsDialog } from "./SecretsDialog";
import { SchemaPanel } from "./SchemaPanel";
import { useConnectionSchema } from "./schema";
import { Sidebar } from "./Sidebar";
import {
  headerSummary,
  joinPath,
  parentPath,
  parseHeader,
  type LibrarySettings,
  type ScriptFile,
} from "./library";

const SAMPLE = `from codingpad import dump

print("Hello from CodingPad!")

dump({"name": "Ada", "languages": ["Python", "C#", "Rust"]})

# The last expression is displayed automatically.
6 * 7
`;

const CSHARP_SAMPLE = `Console.WriteLine("Hello from C#!");

new { Name = "Ada", Language = "C#" }.Dump();
await Task.Delay(10);

// A final value-returning call is displayed automatically.
Enumerable.Range(1, 10).Sum();
`;

const SQL_SAMPLE = `-- codingpad: connection=sample

SELECT 1 AS answer;
`;

const SCRIPT_FILTERS = [
  { name: "Python", extensions: ["py"] },
  { name: "C#", extensions: ["cs"] },
  { name: "SQL", extensions: ["sql"] },
  { name: "All files", extensions: ["*"] },
];

const scriptLanguage = (script: Script) => {
  const name = script.path ?? script.name;
  if (/\.py$/i.test(name)) return "python";
  if (/\.cs$/i.test(name)) return "csharp";
  if (/\.sql$/i.test(name)) return "sql";
  return "plaintext";
};

const languageLabel = (language: string) =>
  language === "csharp" ? "C#" : language === "python" ? "Python" : language === "sql" ? "SQL" : "Text";

type Script = {
  id: string;
  name: string;
  path?: string;
  source: string;
  dirty: boolean;
  output: Output[];
  running: boolean;
  status: string;
  version: number;
  updates: Record<string, DumpNode>;
  controlling?: boolean;
  error?: string;
  missingToolchain?: "uv" | "dotnet";
  compilation?: { source: string; diagnostics: CompileDiagnostic[] };
};
function intelligenceLabel(summary: DiagnosticsSummary): string {
  if (summary.message) return summary.message;
  const parts: string[] = [];
  if (summary.errors)
    parts.push(`${summary.errors} error${summary.errors === 1 ? "" : "s"}`);
  if (summary.warnings)
    parts.push(
      `${summary.warnings} warning${summary.warnings === 1 ? "" : "s"}`,
    );
  return parts.length ? parts.join(" · ") : "No type problems";
}

const newScript = (number: number, language = "python"): Script => ({
  id: uniqueId("scratch"),
  name: `Scratch ${number}.${language === "csharp" ? "cs" : language === "sql" ? "sql" : "py"}`,
  source: language === "csharp" ? CSHARP_SAMPLE : language === "sql" ? SQL_SAMPLE : SAMPLE,
  dirty: false,
  output: [],
  running: false,
  status: "Ready",
  version: 0,
  updates: {},
});

const EMPTY_DIAGNOSTICS: CompileDiagnostic[] = [];

export default function App() {
  const [scripts, setScripts] = useState<Script[]>(() => [newScript(1)]);
  const scriptsRef = useRef(scripts);
  scriptsRef.current = scripts;
  const [activeId, setActiveId] = useState(() => scripts[0].id);
  const [intelligence, setIntelligence] = useState<DiagnosticsSummary>({
    errors: 0,
    warnings: 0,
  });
  const [settings, setSettings] = useState<LibrarySettings | null>(null);
  const [libraryError, setLibraryError] = useState<string | undefined>();
  const [refresh, setRefresh] = useState(0);
  const counter = useRef(1);
  const opening = useRef(new Set<string>());
  const [closing, setClosing] = useState(false);
  const [secretsOpen, setSecretsOpen] = useState(false);
  const [connectionsOpen, setConnectionsOpen] = useState(false);
  const [installProgress, setInstallProgress] = useState<string | null>(null);
  const installingToolchain = useRef(false);
  const [toolchainVersion, setToolchainVersion] = useState(0);
  const generation = useRef(new Map<string, number>());
  const running = useRef(new Set<string>());
  const editor = useRef<EditorHandle | null>(null);
  const frames = useRef(new Map<string, number>());
  const cancelFrame = (id: string) => {
    const frame = frames.current.get(id);
    if (frame !== undefined) cancelAnimationFrame(frame);
    frames.current.delete(id);
  };
  useEffect(() => {
    invoke<LibrarySettings | null>("load_library_settings")
      .then((loaded) => {
        if (
          loaded &&
          typeof loaded.libraryPath === "string" &&
          Array.isArray(loaded.pinnedFolders)
        ) {
          setSettings(loaded);
        }
      })
      .catch((error) => setLibraryError(String(error)));
  }, []);
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
  const language = scriptLanguage(active);
  const runnable = language !== "plaintext";
  const compileDiagnostics = active.compilation?.source === active.source
    ? active.compilation.diagnostics
    : EMPTY_DIAGNOSTICS;
  const activeHeader = parseHeader(active.source);
  const connectionName = activeHeader?.settings.connection || undefined;
  const [schemaRevision, setSchemaRevision] = useState(0);
  const schemaState = useConnectionSchema(connectionName, schemaRevision);
  const update = (id: string, change: (script: Script) => Script) =>
    setScripts((all) =>
      all.map((script) => (script.id === id ? change(script) : script)),
    );

  async function runScript(script: Script) {
    if (
      installingToolchain.current ||
      running.current.has(script.id) ||
      script.controlling
    )
      return;
    if (scriptLanguage(script) === "plaintext") {
      update(script.id, (item) => ({
        ...item,
        status: "Not runnable",
        error: "Only Python, C# and SQL scripts run in this build.",
      }));
      return;
    }
    if (scriptLanguage(script) === "sql") {
      await runSqlScript(script);
      return;
    }
    running.current.add(script.id);
    const version = (generation.current.get(script.id) ?? 0) + 1;
    generation.current.set(script.id, version);
    cancelFrame(script.id);
    update(script.id, (item) => ({
      ...item,
      output: [],
      running: true,
      status: "Running…",
      missingToolchain: undefined,
      version,
      updates: {},
      compilation: undefined,
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
      if (event.method === "diagnostics") {
        update(script.id, (item) => ({
          ...item,
          compilation: { source: script.source, diagnostics: event.params.diagnostics },
        }));
        return;
      }
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
        language: scriptLanguage(script),
        path: script.path ?? null,
        onEvent,
      });
      update(script.id, (item) => ({
        ...item,
        status: result.cacheCleared
          ? "Stopped · cache cleared"
          : result.stopped
            ? "Stopped"
            : result.ok
              ? "Finished"
              : "Script error",
      }));
    } catch (error) {
      const message = String(error);
      update(script.id, (item) => ({
        ...item,
        status: "Could not run",
        error: message,
        missingToolchain: message.startsWith("uv was not found.")
          ? "uv"
          : message.startsWith(".NET was not found.")
            ? "dotnet"
            : undefined,
      }));
    } finally {
      flush();
      running.current.delete(script.id);
      update(script.id, (item) => ({ ...item, running: false }));
    }
  }

  async function runSqlScript(script: Script) {
    running.current.add(script.id);
    const version = (generation.current.get(script.id) ?? 0) + 1;
    generation.current.set(script.id, version);
    update(script.id, (item) => ({
      ...item,
      output: [],
      running: true,
      status: "Running…",
      missingToolchain: undefined,
      version,
      updates: {},
      compilation: undefined,
      error: undefined,
    }));
    try {
      await invoke("begin_sql_run", { scriptId: script.id });
      const result = await invoke<{
        ok: boolean;
        stopped?: boolean;
        tables: DumpNode[];
      }>("run_sql", {
        scriptId: script.id,
        source: script.source,
        path: script.path ?? null,
      });
      if (generation.current.get(script.id) !== version) return;
      update(script.id, (item) => ({
        ...item,
        output: result.tables.map((node, index) => ({
          id: index,
          kind: "dump" as const,
          node,
        })),
        status: result.stopped ? "Stopped" : "Finished",
      }));
    } catch (failure) {
      if (generation.current.get(script.id) !== version) return;
      update(script.id, (item) => ({
        ...item,
        status: "Could not run",
        error: String(failure),
      }));
    } finally {
      running.current.delete(script.id);
      update(script.id, (item) => ({ ...item, running: false }));
    }
  }

  async function installToolchainAndRun(
    script: Script,
    toolchain: "uv" | "dotnet",
  ) {
    if (installingToolchain.current) return;
    installingToolchain.current = true;
    setInstallProgress(
      toolchain === "uv"
        ? "Downloading and verifying uv…"
        : "Downloading and verifying the .NET install script…",
    );
    update(script.id, (item) => ({ ...item, error: undefined }));
    const onProgress = new Channel<string>();
    onProgress.onmessage = (message) => {
      if (installingToolchain.current) setInstallProgress(message);
    };
    let installed = false;
    try {
      await invoke(toolchain === "uv" ? "install_uv" : "install_dotnet", {
        onProgress,
      });
      installed = true;
      setToolchainVersion((version) => version + 1);
    } catch (error) {
      update(script.id, (item) => ({
        ...item,
        error: String(error),
        missingToolchain: toolchain,
      }));
    } finally {
      installingToolchain.current = false;
      setInstallProgress(null);
    }
    const current = scriptsRef.current.find((item) => item.id === script.id);
    if (installed && current) await runScript(current);
  }

  async function controlScript(script: Script, action: "stop" | "reset") {
    if (
      script.controlling ||
      (action === "stop" && !running.current.has(script.id))
    )
      return;
    update(script.id, (item) => ({
      ...item,
      controlling: true,
      error: undefined,
      status: action === "stop" ? "Stopping…" : "Resetting…",
    }));
    try {
      await invoke(`${action}_script`, { scriptId: script.id });
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
    const result = await invoke<{ node: DumpNode }>(
      language === "sql" ? "expand_sql" : "expand_node",
      {
        scriptId: active.id,
        handle,
        cursor,
        limit: 100,
      },
    );
    return result.node;
  }

  async function exportTable(
    handle: string,
    format: ExportFormat,
    onProgress: (rows: number, total: number | null) => void,
    signal: AbortSignal,
  ): Promise<ExportSummary | null> {
    const scriptId = active.id;
    const sql = language === "sql";
    const stem = active.name.replace(/\.[^.]*$/, "") || "table";
    const selected = await saveDialog({
      title: format === "csv" ? "Export table to CSV" : "Export table to Excel",
      defaultPath: `${stem}.${format}`,
      filters: [
        format === "csv"
          ? { name: "CSV", extensions: ["csv"] }
          : { name: "Excel workbook", extensions: ["xlsx"] },
      ],
    });
    if (typeof selected !== "string" || !selected || signal.aborted) return null;
    const path = selected.toLowerCase().endsWith(`.${format}`)
      ? selected
      : `${selected}.${format}`;
    const exportId = uniqueId("export");
    const channel = new Channel<{ rows: number; total: number | null }>();
    channel.onmessage = (progress) => onProgress(progress.rows, progress.total);
    const cancel = () => void invoke("cancel_export", { exportId });
    signal.addEventListener("abort", cancel);
    try {
      return await invoke<ExportSummary>("export_table_dump", {
        exportId,
        scriptId,
        handle,
        sql,
        format,
        path,
        onProgress: channel,
      });
    } finally {
      signal.removeEventListener("abort", cancel);
    }
  }

  function openLine(line: number) {
    const current = editor.current;
    if (!current) return;
    current.revealLineInCenter(line);
    current.setPosition({ lineNumber: line, column: 1 });
    current.focus();
  }

  function addScript(language = "python") {
    const script = newScript(++counter.current, language);
    setScripts((all) => [...all, script]);
    setActiveId(script.id);
  }

  async function closeScript(script: Script) {
    if (script.running || script.controlling || closing || scripts.length === 1)
      return;
    if (
      script.dirty &&
      !(await ask(`Discard unsaved changes to ${script.name}?`, {
        title: "Close unsaved script",
        kind: "warning",
      }))
    )
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

  async function openPath(path: string) {
    const existing = scriptsRef.current.find((script) => script.path === path);
    if (existing) {
      setActiveId(existing.id);
      return;
    }
    if (opening.current.has(path)) return;
    opening.current.add(path);
    try {
      const file = await invoke<ScriptFile>("read_script", { path });
      const alreadyOpen = scriptsRef.current.find(
        (script) => script.path === file.path,
      );
      if (alreadyOpen) {
        setActiveId(alreadyOpen.id);
        return;
      }
      const script: Script = {
        id: uniqueId("file"),
        name: file.name,
        path: file.path,
        source: file.source,
        dirty: false,
        output: [],
        running: false,
        status: "Ready",
        version: 0,
        updates: {},
      };
      scriptsRef.current = [...scriptsRef.current, script];
      setScripts((all) =>
        all.some((item) => item.path === file.path) ? all : [...all, script],
      );
      setActiveId(script.id);
      setLibraryError(undefined);
    } catch (error) {
      setLibraryError(String(error));
    } finally {
      opening.current.delete(path);
    }
  }

  async function openFile() {
    const selected = await openDialog({
      multiple: false,
      title: "Open script",
      filters: SCRIPT_FILTERS,
    });
    if (typeof selected === "string") await openPath(selected);
  }

  async function saveScript(script: Script, saveAs = false) {
    // Capture the text that is actually written: the user may keep typing while
    // the dialog is open or the write is in progress.
    const source = script.source;
    let path = script.path;
    if (!path || saveAs) {
      const directory = script.path
        ? parentPath(script.path)
        : settings?.libraryPath;
      const defaultPath = directory
        ? joinPath(directory, script.name)
        : script.name;
      const selected = await saveDialog({
        title: "Save script",
        defaultPath,
        filters: SCRIPT_FILTERS,
      });
      if (typeof selected !== "string" || !selected) return;
      path = selected;
    }
    const collision = scriptsRef.current.find(
      (item) => item.id !== script.id && item.path === path,
    );
    if (collision?.running || collision?.controlling) {
      update(script.id, (item) => ({
        ...item,
        error: `${collision.name} is open in a running tab. Stop it before saving over it.`,
      }));
      return;
    }
    if (
      collision?.dirty &&
      !(await ask(
        `${collision.name} has unsaved changes. Saving here replaces it on disk and closes that tab.`,
        { title: "Replace open tab", kind: "warning" },
      ))
    )
      return;
    try {
      const saved = await invoke<ScriptFile>("write_script", {
        path,
        source,
      });
      const apply = (item: Script): Script =>
        item.id === script.id
          ? {
              ...item,
              path: saved.path,
              name: saved.name,
              dirty: item.source !== source,
              error: undefined,
            }
          : item;
      const next = (all: Script[]) =>
        all.filter((item) => item.id !== collision?.id).map(apply);
      scriptsRef.current = next(scriptsRef.current);
      setScripts(next);
      if (collision) {
        cancelFrame(collision.id);
        generation.current.delete(collision.id);
        running.current.delete(collision.id);
        void invoke("close_script", { scriptId: collision.id });
      }
      setRefresh((value) => value + 1);
      setLibraryError(undefined);
    } catch (error) {
      update(script.id, (item) => ({ ...item, error: String(error) }));
    }
  }

  async function persistSettings(next: LibrarySettings) {
    try {
      await invoke("save_library_settings", { settings: next });
      setSettings(next);
      setRefresh((value) => value + 1);
      setLibraryError(undefined);
    } catch (error) {
      setLibraryError(String(error));
    }
  }

  async function changeLibrary() {
    const selected = await openDialog({
      directory: true,
      multiple: false,
      title: "Choose library folder",
    });
    if (typeof selected !== "string" || !settings) return;
    await persistSettings({
      ...settings,
      libraryPath: selected,
      pinnedFolders: settings.pinnedFolders.filter(
        (folder) => folder !== selected,
      ),
    });
  }

  async function addPinnedFolder() {
    const selected = await openDialog({
      directory: true,
      multiple: false,
      title: "Pin a folder",
    });
    if (typeof selected !== "string" || !settings) return;
    if (selected === settings.libraryPath) return;
    await persistSettings({
      ...settings,
      pinnedFolders: [...new Set([...settings.pinnedFolders, selected])],
    });
  }

  async function removePinnedFolder(path: string) {
    if (!settings) return;
    await persistSettings({
      ...settings,
      pinnedFolders: settings.pinnedFolders.filter((folder) => folder !== path),
    });
  }

  // Editor commands read current state instead of their mount-time closure.
  const runActive = useRef(() => {});
  runActive.current = () => {
    void runScript(active);
  };
  const saveActive = useRef(() => {});
  saveActive.current = () => {
    void saveScript(active);
  };
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    void (async () => {
      try {
        const stop = await getCurrentWindow().onCloseRequested(
          async (event) => {
            if (!scriptsRef.current.some((script) => script.dirty)) return;
            const discard = await ask(
              "Some scripts have unsaved changes. Close CodingPad anyway?",
              { title: "Unsaved changes", kind: "warning" },
            );
            if (!discard) event.preventDefault();
          },
        );
        if (cancelled) stop();
        else unlisten = stop;
      } catch {
        // The browser test environment has no Tauri window to hook.
      }
    })();
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "s") {
        event.preventDefault();
        saveActive.current();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  return (
    <main className="app">
      <header className="toolbar">
        <div className="brand">
          <span className="brand-mark">c.</span>
          <strong>CodingPad</strong>
          <span className="language">{languageLabel(language)}</span>
        </div>
        <div className="run-controls">
          <button
            className="control-button"
            aria-label="Save"
            onClick={() => void saveScript(active)}
          >
            Save
          </button>
          <button
            className="control-button"
            aria-label="Save as"
            onClick={() => void saveScript(active, true)}
          >
            Save as…
          </button>
          <button
            className="control-button"
            aria-label="Connections"
            onClick={() => setConnectionsOpen(true)}
          >
            Connections…
          </button>
          <button
            className="control-button"
            aria-label="Secrets"
            onClick={() => setSecretsOpen(true)}
          >
            Secrets…
          </button>
          <button
            className="run-button"
            disabled={
              !runnable ||
              active.running ||
              active.controlling ||
              installProgress !== null
            }
            title={
              runnable ? undefined : "Only Python, C# and SQL scripts run in this build"
            }
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
      <nav className="tabs" aria-label="Open scripts">
        {scripts.map((script) => (
          <div
            className={`tab ${script.id === activeId ? "active" : ""}`}
            key={script.id}
          >
            <button
              aria-current={script.id === activeId ? "page" : undefined}
              title={script.path}
              onClick={() => setActiveId(script.id)}
            >
              {script.name}
              {script.dirty && (
                <span className="dirty-dot" aria-hidden>
                  {" "}
                  •
                </span>
              )}
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
          onClick={() => addScript()}
        >
          +
        </button>
        <button className="add-tab" aria-label="New C# scratch script" onClick={() => addScript("csharp")}>
          + C#
        </button>
        <button className="add-tab" aria-label="New SQL scratch script" onClick={() => addScript("sql")}>
          + SQL
        </button>
      </nav>
      <div className="main-area">
        <Sidebar
          settings={settings}
          activePath={active.path}
          refresh={refresh}
          onOpen={(path) => void openPath(path)}
          onOpenFile={() => void openFile()}
          onChangeLibrary={() => void changeLibrary()}
          onAddPinned={() => void addPinnedFolder()}
          onRemovePinned={(path) => void removePinnedFolder(path)}
          error={libraryError}
        />
        {connectionName && <SchemaPanel key={connectionName} name={connectionName} {...schemaState} onRefresh={() => setSchemaRevision((value) => value + 1)} />}
        <div className="workspace">
          <section className="editor-pane" aria-label={`${languageLabel(language)} editor`}>
            <div className="pane-heading">
              <span>EDITOR</span>
              <span className="pane-heading-right">
                <span
                  className={`intelligence ${
                    intelligence.message
                      ? ""
                      : intelligence.errors
                        ? "error"
                        : intelligence.warnings
                          ? "warning"
                          : ""
                  }`}
                  role="status"
                  aria-live="polite"
                  title={intelligenceLabel(intelligence)}
                >
                  {intelligenceLabel(intelligence)}
                </span>
                <span>Ctrl / ⌘ + Enter to run · Ctrl / ⌘ + S to save</span>
              </span>
            </div>
            <div className="editor">
              <CodeEditor
                key={`${active.id}:${language}`}
                id={active.id}
                language={language}
                path={active.path}
                compileDiagnostics={compileDiagnostics}
                toolchainVersion={toolchainVersion}
                sqlSchema={schemaState.schema}
                dataRevision={schemaRevision}
                source={active.source}
                onDiagnostics={setIntelligence}
                onChange={(source) =>
                  update(active.id, (script) => ({
                    ...script,
                    source,
                    dirty: true,
                  }))
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
            <div className="results" aria-busy={active.running || installProgress !== null}>
              {installProgress && <p role="status">{installProgress}</p>}
              {active.missingToolchain && (
                <div className="tool-install">
                  <p>
                    {active.missingToolchain === "dotnet"
                      ? "Install the .NET SDK privately for CodingPad to run C# scripts. This needs an internet connection."
                      : "Install uv privately for CodingPad to run Python scripts. This needs an internet connection."}
                  </p>
                  <button
                    className="control-button"
                    disabled={installProgress !== null || active.running}
                    onClick={() =>
                      void installToolchainAndRun(
                        active,
                        active.missingToolchain!,
                      )
                    }
                  >
                    {installProgress
                      ? active.missingToolchain === "dotnet"
                        ? "Installing .NET…"
                        : "Installing uv…"
                      : active.missingToolchain === "dotnet"
                        ? "Install .NET and run"
                        : "Install uv and run"}
                  </button>
                </div>
              )}
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
                      exportTable={exportTable}
                      onOpenLine={openLine}
                    />
                  </div>
                ),
              )}
            </div>
          </section>
        </div>
      </div>
      <ConnectionsDialog
        open={connectionsOpen}
        onClose={() => { setConnectionsOpen(false); setSchemaRevision((value) => value + 1); }}
      />
      <SecretsDialog open={secretsOpen} onClose={() => setSecretsOpen(false)} />
      <footer>
        <span>
          {active.path
            ? active.path
            : `${active.name} · Not saved to the library`}
        </span>
        <span>
          {activeHeader
            ? `Header: ${headerSummary(activeHeader) || activeHeader.line}`
            : `${languageLabel(language)} · Library and files`}
        </span>
      </footer>
    </main>
  );
}

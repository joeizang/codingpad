import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  baseName,
  type FolderEntry,
  type LibrarySettings,
} from "./library";
import {
  groupSamples,
  type Sample,
} from "./samples";

type SidebarProps = {
  settings: LibrarySettings | null;
  samples: Sample[];
  samplesError?: string;
  activePath?: string;
  activeSample?: string;
  refresh: number;
  onOpen: (path: string) => void;
  onOpenSample: (sample: Sample) => void;
  onOpenFile: () => void;
  onChangeLibrary: () => void;
  onAddPinned: () => void;
  onRemovePinned: (path: string) => void;
  error?: string;
};

type Loaded = { entries?: FolderEntry[]; error?: string };

function FolderTree({
  path,
  activePath,
  refresh,
  onOpen,
}: {
  path: string;
  activePath?: string;
  refresh: number;
  onOpen: (path: string) => void;
}) {
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  const previousPath = useRef(path);
  useEffect(() => {
    let cancelled = false;
    // Keep the tree (and its expanded folders) on screen while refreshing, so
    // opening or saving a file does not collapse the branch that holds it.
    if (previousPath.current !== path) {
      previousPath.current = path;
      setLoaded(null);
    }
    invoke<FolderEntry[]>("list_folder", { path })
      .then((entries) => {
        if (!cancelled) setLoaded({ entries });
      })
      .catch((error) => {
        // Keep the previous entries visible, but say why they may be stale.
        if (!cancelled)
          setLoaded((current) => ({
            ...current,
            error: String(error),
          }));
      });
    return () => {
      cancelled = true;
    };
  }, [path, refresh]);

  if (!loaded) return <div className="tree-status">Loading…</div>;
  return (
    <>
      {loaded.error && <div className="tree-error">{loaded.error}</div>}
      {!loaded.entries ? null : loaded.entries.length === 0 ? (
        <div className="tree-status">No files yet</div>
      ) : (
        <ul className="tree">
          {loaded.entries.map((entry) =>
            entry.isDir ? (
              <FolderBranch
                key={entry.path}
                entry={entry}
                activePath={activePath}
                refresh={refresh}
                onOpen={onOpen}
              />
            ) : (
              <li key={entry.path}>
                <button
                  className={`tree-file ${entry.path === activePath ? "active" : ""}`}
                  aria-current={entry.path === activePath ? "page" : undefined}
                  title={entry.path}
                  onClick={() => onOpen(entry.path)}
                >
                  {entry.name}
                </button>
              </li>
            ),
          )}
        </ul>
      )}
    </>
  );
}

function FolderBranch({
  entry,
  activePath,
  refresh,
  onOpen,
}: {
  entry: FolderEntry;
  activePath?: string;
  refresh: number;
  onOpen: (path: string) => void;
}) {
  const [open, setOpen] = useState(false);
  return (
    <li>
      <button
        className="tree-folder"
        aria-expanded={open}
        title={entry.path}
        onClick={() => setOpen((current) => !current)}
      >
        <span className="tree-caret">{open ? "▾" : "▸"}</span>
        {entry.name}
      </button>
      {open && (
        <div className="tree-children">
          <FolderTree
            path={entry.path}
            activePath={activePath}
            refresh={refresh}
            onOpen={onOpen}
          />
        </div>
      )}
    </li>
  );
}

function SampleGroup({
  label,
  samples,
  activeSample,
  defaultOpen,
  onOpen,
}: {
  label: string;
  samples: Sample[];
  activeSample?: string;
  defaultOpen: boolean;
  onOpen: (sample: Sample) => void;
}) {
  const [open, setOpen] = useState(defaultOpen);
  return (
    <div className="sample-group">
      <button
        className="sample-heading"
        aria-expanded={open}
        onClick={() => setOpen((current) => !current)}
      >
        <span className="tree-caret">{open ? "▾" : "▸"}</span>
        {label}
        <span className="sample-count">{samples.length}</span>
      </button>
      {open && (
        <ul className="tree">
          {samples.map((sample) => (
            <li key={sample.id}>
              <button
                className={`tree-file ${sample.id === activeSample ? "active" : ""}`}
                aria-current={sample.id === activeSample ? "page" : undefined}
                title={`${sample.title} · ${sample.name}`}
                onClick={() => onOpen(sample)}
              >
                {sample.title}
                <span className="sample-file">{sample.name}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

export function Sidebar({
  settings,
  samples,
  samplesError,
  activePath,
  activeSample,
  refresh,
  onOpen,
  onOpenSample,
  onOpenFile,
  onChangeLibrary,
  onAddPinned,
  onRemovePinned,
  error,
}: SidebarProps) {
  return (
    <aside className="sidebar" aria-label="Library">
      <div className="sidebar-actions">
        <button onClick={onOpenFile}>Open file…</button>
      </div>
      {error && (
        <div className="sidebar-error">
          {error}
        </div>
      )}
      {(samples.length > 0 || samplesError) && (
        <section className="sidebar-section">
          <h2>Samples</h2>
          {samplesError ? (
            <div className="tree-error" role="alert">
              {samplesError}
            </div>
          ) : (
            <div className="tree-status">Opening one makes an unsaved copy.</div>
          )}
          {groupSamples(samples).map((group) => (
            <SampleGroup
              key={group.language}
              label={group.label}
              samples={group.samples}
              activeSample={activeSample}
              defaultOpen={group.language === "python"}
              onOpen={onOpenSample}
            />
          ))}
        </section>
      )}
      {!settings ? (
        <div className="tree-status">Loading library…</div>
      ) : (
        <>
          <section className="sidebar-section">
            <h2>
              Library
              <button
                className="section-action"
                aria-label="Change library folder"
                onClick={onChangeLibrary}
              >
                Change…
              </button>
            </h2>
            <div className="folder-label" title={settings.libraryPath}>
              {baseName(settings.libraryPath) || settings.libraryPath}
            </div>
            <FolderTree
              path={settings.libraryPath}
              activePath={activePath}
              refresh={refresh}
              onOpen={onOpen}
            />
          </section>
          <section className="sidebar-section">
            <h2>
              Pinned folders
              <button
                className="section-action"
                aria-label="Pin folder"
                onClick={onAddPinned}
              >
                + Add
              </button>
            </h2>
            {settings.pinnedFolders.length === 0 && (
              <div className="tree-status">No pinned folders</div>
            )}
            {settings.pinnedFolders.map((path) => (
              <div className="pinned-folder" key={path}>
                <div className="folder-label" title={path}>
                  <span>{baseName(path) || path}</span>
                  <button
                    className="section-action"
                    aria-label={`Unpin ${baseName(path) || path}`}
                    onClick={() => onRemovePinned(path)}
                  >
                    ×
                  </button>
                </div>
                <FolderTree
                  path={path}
                  activePath={activePath}
                  refresh={refresh}
                  onOpen={onOpen}
                />
              </div>
            ))}
          </section>
        </>
      )}
    </aside>
  );
}

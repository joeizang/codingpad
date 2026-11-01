import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  baseName,
  type FolderEntry,
  type LibrarySettings,
} from "./library";

type SidebarProps = {
  settings: LibrarySettings | null;
  activePath?: string;
  refresh: number;
  onOpen: (path: string) => void;
  onOpenFile: () => void;
  onChangeLibrary: () => void;
  onAddPinned: () => void;
  onRemovePinned: (path: string) => void;
  error?: string;
};

type Loaded = { entries: FolderEntry[] } | { error: string };

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
  useEffect(() => {
    let cancelled = false;
    setLoaded(null);
    invoke<FolderEntry[]>("list_folder", { path })
      .then((entries) => {
        if (!cancelled) setLoaded({ entries });
      })
      .catch((error) => {
        if (!cancelled) setLoaded({ error: String(error) });
      });
    return () => {
      cancelled = true;
    };
  }, [path, refresh]);

  if (!loaded) return <div className="tree-status">Loading…</div>;
  if ("error" in loaded) return <div className="tree-error">{loaded.error}</div>;
  if (!loaded.entries.length)
    return <div className="tree-status">No files yet</div>;
  return (
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

export function Sidebar({
  settings,
  activePath,
  refresh,
  onOpen,
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

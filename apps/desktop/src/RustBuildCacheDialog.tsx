import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export function RustBuildCacheDialog({ onClose }: { onClose: () => void }) {
  const dialog = useRef<HTMLElement>(null);
  const [bytes, setBytes] = useState<number | null>(null);
  const [error, setError] = useState("");
  const [clearing, setClearing] = useState(false);

  useEffect(() => {
    dialog.current?.focus();
    let active = true;
    void invoke<number>("rust_build_cache_size").then(
      (size) => { if (active) setBytes(size); },
      (failure) => { if (active) setError(String(failure)); },
    );
    return () => { active = false; };
  }, []);

  async function clear() {
    setClearing(true);
    setError("");
    try {
      await invoke("clear_rust_build_cache");
      setBytes(await invoke<number>("rust_build_cache_size"));
    } catch (failure) {
      setError(String(failure));
    } finally {
      setClearing(false);
    }
  }

  const size = bytes === null ? "" : bytes < 1024
    ? `${bytes.toLocaleString()} B`
    : bytes < 1024 * 1024
      ? `${(bytes / 1024).toLocaleString(undefined, { maximumFractionDigits: 1 })} KiB`
      : `${(bytes / (1024 * 1024)).toLocaleString(undefined, { maximumFractionDigits: 1 })} MiB`;

  return (
    <div className="modal-backdrop" role="presentation" onMouseDown={(event) => {
      if (event.target === event.currentTarget) onClose();
    }}>
      <section
        ref={dialog}
        className="modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="rust-cache-title"
        tabIndex={-1}
        onKeyDown={(event) => { if (event.key === "Escape") onClose(); }}
      >
        <header className="modal-heading">
          <h2 id="rust-cache-title">Rust build cache</h2>
          <button className="section-action" aria-label="Close Rust build cache" onClick={onClose}>×</button>
        </header>
        <p className="modal-note" aria-live="polite">
          {clearing
            ? "Clearing build artifacts…"
            : bytes !== null
              ? `${size} in compiled artifacts`
              : error ? "" : "Measuring build artifacts…"}
        </p>
        {error && <p role="alert">{error}</p>}
        <div className="modal-actions">
          <button disabled={clearing || bytes === 0 || (bytes === null && !error)} onClick={() => void clear()}>
            Clear build cache
          </button>
        </div>
      </section>
    </div>
  );
}

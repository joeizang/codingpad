import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useCrashEnvironment, type CrashDetails } from "./crash";

/**
 * The crash screen. Nothing leaves the machine until the user presses Report on
 * GitHub and submits the reviewed issue in the browser (D31).
 */
export function CrashScreen({
  details,
  onDismiss,
  onReload,
}: {
  details: CrashDetails;
  onDismiss?: () => void;
  onReload?: () => void;
}) {
  const environment = useCrashEnvironment();
  const [opening, setOpening] = useState(false);
  const [error, setError] = useState<string | undefined>();

  async function report() {
    if (opening) return;
    setOpening(true);
    setError(undefined);
    try {
      const url = await invoke<string>("crash_report_url", { details });
      await openUrl(url);
    } catch (failure) {
      setError(`Could not open the report: ${String(failure)}`);
    } finally {
      setOpening(false);
    }
  }

  const technical = [details.location, details.backtrace]
    .filter((part): part is string => Boolean(part && part.trim()))
    .join("\n\n");

  return (
    <div className="crash-backdrop">
      <section
        className="crash"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="crash-title"
        aria-describedby="crash-summary"
      >
        <h2 id="crash-title">CodingPad hit an unexpected error</h2>
        <p id="crash-summary">
          Nothing has been sent. A report opens a pre-filled GitHub issue that
          you review and submit yourself; it holds only the app version, your
          platform and the error below — never a script, connection detail or
          secret.
        </p>
        <pre className="crash-message">{details.message}</pre>
        {technical && (
          <details className="crash-technical">
            <summary>Technical details</summary>
            <pre>{technical}</pre>
          </details>
        )}
        {environment && (
          <p className="crash-environment">
            CodingPad {environment.version} · {environment.platform} (
            {environment.arch})
          </p>
        )}
        {error && (
          <p className="crash-error" role="alert">
            {error}
          </p>
        )}
        <div className="crash-actions">
          <button
            className="run-button"
            disabled={opening}
            onClick={() => void report()}
          >
            {opening ? "Opening GitHub…" : "Report on GitHub"}
          </button>
          {onReload && (
            <button className="control-button" onClick={onReload}>
              Reload
            </button>
          )}
          {onDismiss && (
            <button className="control-button" onClick={onDismiss}>
              Dismiss
            </button>
          )}
        </div>
      </section>
    </div>
  );
}

import { openUrl } from "@tauri-apps/plugin-opener";
import type { AvailableUpdate } from "./updates";

/**
 * Shows the newest release and opens its GitHub page in the system browser.
 * The app window itself never navigates away.
 */
export function UpdateNotice({ update }: { update: AvailableUpdate }) {
  return (
    <div className="update-notice" role="status">
      <span>CodingPad {update.version} is available.</span>
      <a
        className="update-link"
        href={update.url}
        rel="noreferrer"
        onClick={(event) => {
          event.preventDefault();
          // A browser that cannot open still leaves the app untouched.
          void openUrl(update.url).catch(() => {});
        }}
      >
        View release
      </a>
    </div>
  );
}

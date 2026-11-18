import { useState } from "react";
import { resetOncePerLaunchForTests, useOncePerLaunch } from "./oncePerLaunch";

/**
 * The error details a crash report may carry. CodingPad never reads scripts,
 * connection details or secrets for a report (D31).
 */
export type CrashDetails = {
  message: string;
  location?: string;
  backtrace?: string;
};

/** The running build, shown on the crash screen and in the issue. */
export type CrashEnvironment = {
  version: string;
  platform: string;
  arch: string;
};

/** Only issue forms in this repository are ever opened. */
export const NEW_ISSUE_PREFIX =
  "https://github.com/joeizang/codingpad/issues/";

/** Accept only a well-formed crash from the Rust side. */
export function isCrashDetails(value: unknown): value is CrashDetails {
  if (typeof value !== "object" || value === null) return false;
  const details = value as Partial<CrashDetails>;
  if (typeof details.message !== "string" || details.message.trim() === "")
    return false;
  for (const field of [details.location, details.backtrace]) {
    if (field !== undefined && typeof field !== "string") return false;
  }
  return true;
}

/** Accept only a well-formed environment from the Rust side. */
export function isCrashEnvironment(value: unknown): value is CrashEnvironment {
  if (typeof value !== "object" || value === null) return false;
  const environment = value as Partial<CrashEnvironment>;
  return (
    typeof environment.version === "string" &&
    environment.version.length > 0 &&
    typeof environment.platform === "string" &&
    environment.platform.length > 0 &&
    typeof environment.arch === "string" &&
    environment.arch.length > 0
  );
}

/** Accept only a pre-filled issue in this repository. */
export function isIssueUrl(value: unknown): value is string {
  return typeof value === "string" && value.startsWith(NEW_ISSUE_PREFIX);
}

/** The error details of a value an error boundary caught. */
export function crashDetailsFrom(error: unknown): CrashDetails {
  if (error instanceof Error) {
    const message = error.message.trim() || error.name.trim();
    return {
      message: message || "CodingPad hit an unexpected error.",
      backtrace: error.stack,
    };
  }
  if (typeof error === "string" && error.trim())
    return { message: error.trim() };
  return { message: "CodingPad hit an unexpected error." };
}

/**
 * A crash left by an earlier panic, offered once on the next launch. Dismissing
 * it only hides the screen; the record was already taken.
 */
export function usePendingCrash(): [CrashDetails | null, () => void] {
  const pending = useOncePerLaunch("take_pending_crash", isCrashDetails);
  const [dismissed, setDismissed] = useState(false);
  return [dismissed ? null : pending, () => setDismissed(true)];
}

/** The running build, for the crash screen. A failure shows no line. */
export function useCrashEnvironment(): CrashEnvironment | null {
  return useOncePerLaunch("crash_environment", isCrashEnvironment);
}

/** Forget the shared reads so each test starts clean. Tests only. */
export function resetPendingCrashForTests(): void {
  resetOncePerLaunchForTests();
}

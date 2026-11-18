import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

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
 * One shared read per launch. StrictMode mounts effects twice in development;
 * taking the pending crash is destructive, so the second mount must join the
 * first rather than look again and find nothing. Tests forget it between cases.
 */
let pendingRequest: Promise<unknown> | undefined;

function requestPendingCrash(): Promise<unknown> {
  pendingRequest ??= Promise.resolve(invoke<unknown>("take_pending_crash"));
  return pendingRequest;
}

/** Forget the shared read so each test starts clean. Tests only. */
export function resetPendingCrashForTests(): void {
  pendingRequest = undefined;
}

/**
 * A crash left by an earlier panic, offered once on the next launch. A failed
 * read leaves the app exactly as it was.
 */
export function usePendingCrash(): [CrashDetails | null, () => void] {
  const [pending, setPending] = useState<CrashDetails | null>(null);
  useEffect(() => {
    let current = true;
    void requestPendingCrash().then(
      (found) => {
        if (current && isCrashDetails(found)) setPending(found);
      },
      () => {},
    );
    return () => {
      current = false;
    };
  }, []);
  return [pending, () => setPending(null)];
}

/** The running build, for the crash screen. A failure shows no line. */
export function useCrashEnvironment(): CrashEnvironment | null {
  const [environment, setEnvironment] = useState<CrashEnvironment | null>(null);
  useEffect(() => {
    let current = true;
    void Promise.resolve(invoke<unknown>("crash_environment")).then(
      (found) => {
        if (current && isCrashEnvironment(found)) setEnvironment(found);
      },
      () => {},
    );
    return () => {
      current = false;
    };
  }, []);
  return environment;
}

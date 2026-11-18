import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

/** A release newer than this build, ready for the notice. */
export type AvailableUpdate = { version: string; url: string };

/** Only release pages in this repository are ever offered or opened. */
export const RELEASE_PAGE_PREFIX =
  "https://github.com/joeizang/codingpad/releases/";

/** Accept only a well-formed notice from the Rust side. */
export function isAvailableUpdate(value: unknown): value is AvailableUpdate {
  if (typeof value !== "object" || value === null) return false;
  const update = value as Partial<AvailableUpdate>;
  return (
    typeof update.version === "string" &&
    update.version.length > 0 &&
    typeof update.url === "string" &&
    update.url.startsWith(RELEASE_PAGE_PREFIX)
  );
}

/**
 * One shared request per launch. React StrictMode runs effects twice in
 * development, so the second run joins the first instead of asking GitHub
 * again. The cache is dropped once the answer arrives; a later mount in this
 * process (in practice only a test) starts a fresh check.
 */
let pending: Promise<unknown> | undefined;

function requestUpdate(): Promise<unknown> {
  pending ??= invoke<unknown>("check_for_update").finally(() => {
    pending = undefined;
  });
  return pending;
}

/** Forget a shared check so each test starts clean. Tests only. */
export function resetUpdateCheckForTests(): void {
  pending = undefined;
}

/**
 * Ask GitHub once per launch whether a newer release exists. This is the only
 * request CodingPad makes on its own (D29, D31). A failed check, an offline
 * machine and a current build all leave the app exactly as it was.
 */
export function useAvailableUpdate(): AvailableUpdate | null {
  const [update, setUpdate] = useState<AvailableUpdate | null>(null);
  useEffect(() => {
    let current = true;
    void requestUpdate().then(
      (found) => {
        if (current && isAvailableUpdate(found)) setUpdate(found);
      },
      // A failed check stays silent: no error, no dialog, no retry.
      () => {},
    );
    return () => {
      current = false;
    };
  }, []);
  return update;
}

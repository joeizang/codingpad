import { useOncePerLaunch, resetOncePerLaunchForTests } from "./oncePerLaunch";

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
 * Ask GitHub once per launch whether a newer release exists. This is the only
 * request CodingPad makes on its own (D29, D31). A failed check, an offline
 * machine and a current build all leave the app exactly as it was.
 */
export function useAvailableUpdate(): AvailableUpdate | null {
  return useOncePerLaunch("check_for_update", isAvailableUpdate);
}

/** Forget the shared check so each test starts clean. Tests only. */
export function resetUpdateCheckForTests(): void {
  resetOncePerLaunchForTests();
}

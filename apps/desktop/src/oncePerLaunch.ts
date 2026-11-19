import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

/**
 * One shared request per launch. React StrictMode runs effects twice in
 * development, and a destructive command must not run twice, so the second
 * mount joins the first instead of asking again. A failed request stays silent
 * and leaves the caller unchanged.
 */
const requests = new Map<string, Promise<unknown>>();

function request(command: string): Promise<unknown> {
  let pending = requests.get(command);
  if (!pending) {
    pending = Promise.resolve(invoke<unknown>(command));
    requests.set(command, pending);
  }
  return pending;
}

/** Forget the shared requests so each test starts clean. Tests only. */
export function resetOncePerLaunchForTests(): void {
  requests.clear();
}

export function useOncePerLaunch<T>(
  command: string,
  isValid: (value: unknown) => value is T,
): T | null {
  const [value, setValue] = useState<T | null>(null);
  useEffect(() => {
    let current = true;
    void request(command).then(
      (found) => {
        if (current && isValid(found)) setValue(found);
      },
      () => {},
    );
    return () => {
      current = false;
    };
  }, [command, isValid]);
  return value;
}

import { renderHook, waitFor } from "@testing-library/react";
import { beforeEach, expect, test, vi } from "vitest";
import {
  resetOncePerLaunchForTests,
  useOncePerLaunch,
} from "./oncePerLaunch";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

const isText = (value: unknown): value is string => typeof value === "string";

beforeEach(() => {
  invoke.mockReset();
  resetOncePerLaunchForTests();
});

test("one shared request serves every mount", async () => {
  invoke.mockResolvedValue("ready");
  const first = renderHook(() => useOncePerLaunch("probe", isText));
  const second = renderHook(() => useOncePerLaunch("probe", isText));
  expect(invoke).toHaveBeenCalledTimes(1);
  await waitFor(() => expect(second.result.current).toBe("ready"));
  expect(first.result.current).toBe("ready");
});

test("a failed request stays silent", async () => {
  invoke.mockRejectedValue(new Error("offline"));
  const { result } = renderHook(() => useOncePerLaunch("probe", isText));
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("probe"));
  expect(result.current).toBeNull();
});

test("an unexpected answer is ignored", async () => {
  invoke.mockResolvedValue(42);
  const { result } = renderHook(() => useOncePerLaunch("probe", isText));
  await waitFor(() => expect(invoke).toHaveBeenCalled());
  expect(result.current).toBeNull();
});

test("resetting lets the next case ask again", async () => {
  invoke.mockResolvedValue("first");
  const first = renderHook(() => useOncePerLaunch("probe", isText));
  await waitFor(() => expect(first.result.current).toBe("first"));
  resetOncePerLaunchForTests();
  invoke.mockResolvedValue("second");
  const second = renderHook(() => useOncePerLaunch("probe", isText));
  await waitFor(() => expect(second.result.current).toBe("second"));
  expect(invoke).toHaveBeenCalledTimes(2);
});

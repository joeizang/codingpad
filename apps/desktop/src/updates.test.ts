import { renderHook, waitFor } from "@testing-library/react";
import { beforeEach, expect, test, vi } from "vitest";
import { isAvailableUpdate, useAvailableUpdate } from "./updates";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

const RELEASE = "https://github.com/joeizang/codingpad/releases/tag/v0.2.0";

beforeEach(() => {
  invoke.mockReset();
});

test("a newer release becomes the notice", async () => {
  invoke.mockResolvedValue({ version: "0.2.0", url: RELEASE });
  const { result } = renderHook(() => useAvailableUpdate());
  await waitFor(() =>
    expect(result.current).toEqual({ version: "0.2.0", url: RELEASE }),
  );
  expect(invoke).toHaveBeenCalledWith("check_for_update");
});

test("a current build shows nothing", async () => {
  invoke.mockResolvedValue(null);
  const { result } = renderHook(() => useAvailableUpdate());
  await waitFor(() => expect(invoke).toHaveBeenCalled());
  expect(result.current).toBeNull();
});

test("a failed check stays silent", async () => {
  invoke.mockRejectedValue(new Error("offline"));
  const { result } = renderHook(() => useAvailableUpdate());
  await waitFor(() => expect(invoke).toHaveBeenCalled());
  expect(result.current).toBeNull();
});

test("only release pages in this repository are accepted", () => {
  expect(isAvailableUpdate({ version: "0.2.0", url: RELEASE })).toBe(true);
  expect(isAvailableUpdate({ version: "", url: RELEASE })).toBe(false);
  expect(
    isAvailableUpdate({ version: "0.2.0", url: "https://example.com/release" }),
  ).toBe(false);
  expect(
    isAvailableUpdate({
      version: "0.2.0",
      url: "https://github.com/someone/codingpad/releases/tag/v0.2.0",
    }),
  ).toBe(false);
  expect(isAvailableUpdate({ version: "0.2.0" })).toBe(false);
  expect(isAvailableUpdate(null)).toBe(false);
});

import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, expect, test, vi } from "vitest";
import {
  crashDetailsFrom,
  isCrashDetails,
  isIssueUrl,
  resetPendingCrashForTests,
  usePendingCrash,
} from "./crash";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

beforeEach(() => {
  invoke.mockReset();
  resetPendingCrashForTests();
});

test("a crash left by an earlier panic is offered once", async () => {
  invoke.mockResolvedValue({
    message: "CodingPad was asked to crash",
    location: "main.rs:42:9",
  });
  const { result } = renderHook(() => usePendingCrash());
  await waitFor(() =>
    expect(result.current[0]).toEqual({
      message: "CodingPad was asked to crash",
      location: "main.rs:42:9",
    }),
  );
  act(() => result.current[1]());
  expect(result.current[0]).toBeNull();
});

test("no pending crash leaves the app untouched", async () => {
  invoke.mockResolvedValue(null);
  const { result } = renderHook(() => usePendingCrash());
  await waitFor(() => expect(invoke).toHaveBeenCalledWith("take_pending_crash"));
  expect(result.current[0]).toBeNull();
});

test("a failed read stays silent", async () => {
  invoke.mockRejectedValue(new Error("no app data folder"));
  const { result } = renderHook(() => usePendingCrash());
  await waitFor(() => expect(invoke).toHaveBeenCalled());
  expect(result.current[0]).toBeNull();
});

test("a malformed crash is ignored", async () => {
  invoke.mockResolvedValue({ location: "main.rs:42:9" });
  const { result } = renderHook(() => usePendingCrash());
  await waitFor(() => expect(invoke).toHaveBeenCalled());
  expect(result.current[0]).toBeNull();
});

test("a doubled mount, as StrictMode does in development, reads once", async () => {
  invoke.mockResolvedValue({ message: "boom" });
  const first = renderHook(() => usePendingCrash());
  const second = renderHook(() => usePendingCrash());
  expect(invoke).toHaveBeenCalledTimes(1);
  await waitFor(() => expect(second.result.current[0]).toEqual({ message: "boom" }));
  expect(first.result.current[0]).toEqual({ message: "boom" });
});

test("only a message with optional strings is a crash", () => {
  expect(isCrashDetails({ message: "boom" })).toBe(true);
  expect(
    isCrashDetails({ message: "boom", location: "a.rs:1", backtrace: "at x" }),
  ).toBe(true);
  expect(isCrashDetails({ message: "  " })).toBe(false);
  expect(isCrashDetails({ message: 42 })).toBe(false);
  expect(isCrashDetails({ message: "boom", backtrace: 3 })).toBe(false);
  expect(isCrashDetails(null)).toBe(false);
});

test("only issue forms in this repository are accepted", () => {
  const issue =
    "https://github.com/joeizang/codingpad/issues/new?title=Crash+report";
  expect(isIssueUrl(issue)).toBe(true);
  expect(isIssueUrl("https://example.com/issues/new")).toBe(false);
  expect(
    isIssueUrl("https://github.com/someone/codingpad/issues/new"),
  ).toBe(false);
  expect(isIssueUrl(null)).toBe(false);
});

test("an error becomes the details a report needs", () => {
  const error = new Error("the window could not be created");
  expect(crashDetailsFrom(error)).toEqual({
    message: "the window could not be created",
    backtrace: error.stack,
  });
  expect(crashDetailsFrom("plain failure")).toEqual({
    message: "plain failure",
  });
  expect(crashDetailsFrom(42)).toEqual({
    message: "CodingPad hit an unexpected error.",
  });
});

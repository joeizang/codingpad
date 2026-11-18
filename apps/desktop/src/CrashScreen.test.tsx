import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, test, vi } from "vitest";
import { CrashScreen } from "./CrashScreen";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
const { openUrl } = vi.hoisted(() => ({ openUrl: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl }));

const ISSUE =
  "https://github.com/joeizang/codingpad/issues/new?title=Crash+report%3A+boom";

beforeEach(() => {
  invoke.mockReset();
  openUrl.mockReset();
  openUrl.mockResolvedValue(undefined);
  invoke.mockImplementation(async (command: string) => {
    if (command === "crash_environment")
      return { version: "0.1.0", platform: "linux", arch: "x86_64" };
    if (command === "crash_report_url") return ISSUE;
    return undefined;
  });
});

test("the crash screen names the error and the build, and sends nothing", async () => {
  render(<CrashScreen details={{ message: "boom" }} />);
  expect(
    screen.getByRole("heading", { name: "CodingPad hit an unexpected error" }),
  ).toBeInTheDocument();
  expect(screen.getByText("boom")).toBeInTheDocument();
  expect(
    await screen.findByText("CodingPad 0.1.0 · linux (x86_64)"),
  ).toBeInTheDocument();
  expect(openUrl).not.toHaveBeenCalled();
});

test("Report on GitHub opens the reviewed issue in the browser", async () => {
  render(
    <CrashScreen
      details={{ message: "boom", location: "main.rs:42:9" }}
    />,
  );
  fireEvent.click(screen.getByRole("button", { name: "Report on GitHub" }));
  await waitFor(() => expect(openUrl).toHaveBeenCalledWith(ISSUE));
  expect(invoke).toHaveBeenCalledWith("crash_report_url", {
    details: { message: "boom", location: "main.rs:42:9" },
  });
});

test("a browser that cannot open shows why", async () => {
  openUrl.mockRejectedValue(new Error("no browser"));
  render(<CrashScreen details={{ message: "boom" }} />);
  fireEvent.click(screen.getByRole("button", { name: "Report on GitHub" }));
  expect(await screen.findByRole("alert")).toHaveTextContent(
    "Could not open the report",
  );
});

test("technical details sit behind a summary", () => {
  render(
    <CrashScreen
      details={{ message: "boom", location: "main.rs:42:9", backtrace: "at x" }}
    />,
  );
  expect(screen.getByText("Technical details").tagName).toBe("SUMMARY");
  const details = screen.getByText(/main\.rs:42:9/).closest("details");
  expect(details).not.toBeNull();
});

test("dismiss and reload do what they say", () => {
  const onDismiss = vi.fn();
  const onReload = vi.fn();
  render(
    <CrashScreen details={{ message: "boom" }} onDismiss={onDismiss} onReload={onReload} />,
  );
  fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
  fireEvent.click(screen.getByRole("button", { name: "Reload" }));
  expect(onDismiss).toHaveBeenCalledTimes(1);
  expect(onReload).toHaveBeenCalledTimes(1);
});

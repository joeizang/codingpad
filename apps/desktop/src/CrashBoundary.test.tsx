import { render, screen } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { CrashBoundary } from "./CrashBoundary";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

function Bomb(): never {
  throw new Error("the interface could not be drawn");
}

test("an error while drawing offers the same report as a native crash", () => {
  // React logs caught boundary errors; the test only cares about the screen.
  const logged = vi.spyOn(console, "error").mockImplementation(() => {});
  render(
    <CrashBoundary>
      <Bomb />
    </CrashBoundary>,
  );
  expect(
    screen.getByRole("heading", { name: "CodingPad hit an unexpected error" }),
  ).toBeInTheDocument();
  expect(
    screen.getByText("the interface could not be drawn"),
  ).toBeInTheDocument();
  expect(
    screen.getByRole("button", { name: "Report on GitHub" }),
  ).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Reload" })).toBeInTheDocument();
  logged.mockRestore();
});

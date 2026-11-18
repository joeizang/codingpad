import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, expect, test, vi } from "vitest";
import { UpdateNotice } from "./UpdateNotice";

const { openUrl } = vi.hoisted(() => ({ openUrl: vi.fn() }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl }));

const RELEASE = "https://github.com/joeizang/codingpad/releases/tag/v0.2.0";

beforeEach(() => {
  openUrl.mockReset();
  openUrl.mockResolvedValue(undefined);
});

test("the notice names the release and links to its page", () => {
  render(<UpdateNotice update={{ version: "0.2.0", url: RELEASE }} />);
  expect(screen.getByText("CodingPad 0.2.0 is available.")).toBeInTheDocument();
  expect(screen.getByRole("link", { name: "View release" })).toHaveAttribute(
    "href",
    RELEASE,
  );
});

test("opening the release uses the system browser, not the app window", () => {
  render(<UpdateNotice update={{ version: "0.2.0", url: RELEASE }} />);
  fireEvent.click(screen.getByRole("link", { name: "View release" }));
  expect(openUrl).toHaveBeenCalledWith(RELEASE);
});

test("a browser that cannot open leaves the app untouched", () => {
  openUrl.mockRejectedValue(new Error("no browser"));
  render(<UpdateNotice update={{ version: "0.2.0", url: RELEASE }} />);
  fireEvent.click(screen.getByRole("link", { name: "View release" }));
  expect(openUrl).toHaveBeenCalledWith(RELEASE);
});

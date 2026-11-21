import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { describe, expect, it, vi } from "vitest";
import { RustBuildCacheDialog } from "./RustBuildCacheDialog";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

describe("Rust build cache", () => {
  it("shows compiled size and the empty state after clearing", async () => {
    vi.mocked(invoke)
      .mockResolvedValueOnce(2048)
      .mockResolvedValueOnce(undefined)
      .mockResolvedValueOnce(0);
    render(<RustBuildCacheDialog onClose={vi.fn()} />);
    expect(await screen.findByText("2 KiB in compiled artifacts")).toBeVisible();
    const clear = screen.getByRole("button", { name: "Clear build cache" });
    fireEvent.click(clear);
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("clear_rust_build_cache"));
    expect(await screen.findByText("0 B in compiled artifacts")).toBeVisible();
    expect(clear).toBeDisabled();
  });
});

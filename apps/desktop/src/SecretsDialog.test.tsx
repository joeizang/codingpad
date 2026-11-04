import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, test, vi } from "vitest";
import { SecretsDialog } from "./SecretsDialog";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

beforeEach(() => {
  invoke.mockReset();
});

test("lists, adds and removes named secrets without showing values", async () => {
  let names = ["api-key"];
  invoke.mockImplementation(async (command: string, args?: { name?: string }) => {
    if (command === "list_secrets") return names;
    if (command === "vault_available") return true;
    if (command === "save_secret") {
      names = [...new Set([...names, args!.name!])];
      return "vault";
    }
    if (command === "delete_secret") {
      names = names.filter((name) => name !== args!.name);
      return undefined;
    }
    return undefined;
  });
  render(<SecretsDialog open onClose={() => {}} />);
  expect(await screen.findByRole("button", { name: "api-key" })).toBeInTheDocument();
  fireEvent.change(screen.getByLabelText("Name"), { target: { value: "database" } });
  fireEvent.change(screen.getByLabelText("Value"), { target: { value: "secret" } });
  fireEvent.click(screen.getByRole("button", { name: "Add secret" }));
  await waitFor(() =>
    expect(screen.getByRole("status")).toHaveTextContent("OS password vault"),
  );
  expect(invoke).toHaveBeenCalledWith("save_secret", {
    name: "database",
    value: "secret",
  });
  expect(await screen.findByRole("button", { name: "database" })).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Remove api-key" }));
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("delete_secret", { name: "api-key" }),
  );
});

test("explains session-only storage when the vault is unavailable", async () => {
  invoke.mockImplementation(async (command: string) => {
    if (command === "list_secrets") return [];
    if (command === "vault_available") return false;
    if (command === "save_secret") return "session";
    return undefined;
  });
  render(<SecretsDialog open onClose={() => {}} />);
  expect(await screen.findByText(/memory for this session/)).toBeInTheDocument();
  fireEvent.change(screen.getByLabelText("Name"), { target: { value: "token" } });
  fireEvent.change(screen.getByLabelText("Value"), { target: { value: "value" } });
  fireEvent.click(screen.getByRole("button", { name: "Add secret" }));
  await waitFor(() =>
    expect(screen.getByRole("status")).toHaveTextContent("session only"),
  );
});

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, test, vi } from "vitest";
import { SecretsDialog } from "./SecretsDialog";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

beforeEach(() => {
  invoke.mockReset();
});

test("lists, adds and removes named secrets without showing values", async () => {
  let names = [{ name: "api-key", origin: "vault" }];
  invoke.mockImplementation(async (command: string, args?: { name?: string }) => {
    if (command === "list_secrets") return names;
    if (command === "vault_available") return true;
    if (command === "save_secret") {
      names = [...names.filter((entry) => entry.name !== args!.name), { name: args!.name!, origin: "vault" }];
      return "vault";
    }
    if (command === "delete_secret") {
      names = names.filter((entry) => entry.name !== args!.name);
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
    sessionOnly: false,
  });
  expect(await screen.findByRole("button", { name: "database" })).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Remove api-key" }));
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("delete_secret", {
      name: "api-key",
      sessionOnly: false,
    }),
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
  expect(await screen.findByText(/unavailable.*session only/)).toBeInTheDocument();
  fireEvent.change(screen.getByLabelText("Name"), { target: { value: "token" } });
  fireEvent.change(screen.getByLabelText("Value"), { target: { value: "value" } });
  fireEvent.click(screen.getByRole("button", { name: "Add secret" }));
  await waitFor(() =>
    expect(screen.getByRole("status")).toHaveTextContent("session only"),
  );
});

test("offers an explicit session-only save when the vault fails", async () => {
  invoke.mockImplementation(async (command: string, args?: any) => {
    if (command === "list_secrets") return [];
    if (command === "vault_available") return null;
    if (command === "save_secret") {
      if (args.sessionOnly) return "session";
      throw new Error("The OS password vault is unavailable: locked");
    }
    return undefined;
  });
  render(<SecretsDialog open onClose={() => {}} />);
  fireEvent.change(screen.getByLabelText("Name"), { target: { value: "token" } });
  fireEvent.change(screen.getByLabelText("Value"), { target: { value: "value" } });
  fireEvent.click(screen.getByRole("button", { name: "Add secret" }));
  await waitFor(() =>
    expect(screen.getByRole("alert")).toHaveTextContent("locked"),
  );
  fireEvent.click(
    screen.getByRole("button", { name: "Store for this session only" }),
  );
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("save_secret", {
      name: "token",
      value: "value",
      sessionOnly: true,
    }),
  );
});

test("does not offer session storage for non-vault errors", async () => {
  invoke.mockImplementation(async (command: string) => {
    if (command === "list_secrets") return [];
    if (command === "vault_available") return true;
    if (command === "save_secret") throw new Error("A secret name cannot be empty");
    return undefined;
  });
  render(<SecretsDialog open onClose={() => {}} />);
  fireEvent.change(screen.getByLabelText("Name"), { target: { value: "token" } });
  fireEvent.change(screen.getByLabelText("Value"), { target: { value: "value" } });
  fireEvent.click(screen.getByRole("button", { name: "Add secret" }));
  await waitFor(() =>
    expect(screen.getByRole("alert")).toHaveTextContent("cannot be empty"),
  );
  expect(
    screen.queryByRole("button", { name: "Store for this session only" }),
  ).not.toBeInTheDocument();
});

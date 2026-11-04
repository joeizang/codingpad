import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, test, vi } from "vitest";
import { ConnectionsDialog } from "./ConnectionsDialog";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

beforeEach(() => {
  invoke.mockReset();
});

test("lists, adds, tests and removes SQLite connections", async () => {
  let connections = [
    { id: "one", name: "sample", kind: "sqlite", path: "/tmp/sample.db" },
  ];
  invoke.mockImplementation(async (command: string, args?: any) => {
    if (command === "list_connections") return connections;
    if (command === "save_connection") {
      const saved = { ...args.connection, id: args.connection.id || "two" };
      connections = [
        ...connections.filter((item) => item.id !== saved.id),
        saved,
      ];
      return saved;
    }
    if (command === "test_connection") return "Connected to the SQLite database.";
    if (command === "delete_connection") {
      connections = connections.filter((item) => item.id !== args.id);
      return undefined;
    }
    return undefined;
  });
  render(<ConnectionsDialog open onClose={() => {}} />);
  expect(await screen.findByRole("button", { name: /^sample/ })).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Test" }));
  await waitFor(() =>
    expect(screen.getByRole("status")).toHaveTextContent("Connected"),
  );
  fireEvent.change(screen.getByLabelText("Name"), {
    target: { value: "northwind" },
  });
  fireEvent.change(screen.getByLabelText("Database file"), {
    target: { value: "/tmp/northwind.db" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Add connection" }));
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("save_connection", {
      connection: {
        id: "",
        name: "northwind",
        kind: "sqlite",
        path: "/tmp/northwind.db",
      },
    }),
  );
  expect(await screen.findByRole("button", { name: /^northwind/ })).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Remove sample" }));
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("delete_connection", { id: "one" }),
  );
});

test("rejects connection names with spaces before saving", async () => {
  invoke.mockImplementation(async (command: string) => {
    if (command === "list_connections") return [];
    return undefined;
  });
  render(<ConnectionsDialog open onClose={() => {}} />);
  fireEvent.change(screen.getByLabelText("Name"), {
    target: { value: "my db" },
  });
  fireEvent.change(screen.getByLabelText("Database file"), {
    target: { value: "/tmp/my.db" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Add connection" }));
  await waitFor(() =>
    expect(screen.getByRole("alert")).toHaveTextContent("cannot contain spaces"),
  );
  expect(invoke).not.toHaveBeenCalledWith("save_connection", expect.anything());
});

test("keeps PostgreSQL passwords in the vault and out of connection settings", async () => {
  invoke.mockImplementation(async (command: string, args?: any) => {
    if (command === "list_connections") return [];
    if (command === "save_secret") return "vault";
    if (command === "save_connection") return { ...args.connection, id: "pg" };
    return undefined;
  });
  render(<ConnectionsDialog open onClose={() => {}} />);
  fireEvent.change(screen.getByLabelText("Name"), {
    target: { value: "northwind" },
  });
  fireEvent.change(screen.getByLabelText("Kind"), {
    target: { value: "postgres" },
  });
  fireEvent.change(screen.getByLabelText("Host"), {
    target: { value: "db.example.com" },
  });
  fireEvent.change(screen.getByLabelText("Database"), {
    target: { value: "northwind" },
  });
  fireEvent.change(screen.getByLabelText("User"), {
    target: { value: "reader" },
  });
  fireEvent.change(screen.getByLabelText("Password"), {
    target: { value: "hunter2" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Add connection" }));
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("save_secret", {
      name: "connection:northwind",
      value: "hunter2",
    }),
  );
  const call = invoke.mock.calls.find(([command]) => command === "save_connection")!;
  expect(call[1].connection).toMatchObject({
    name: "northwind",
    kind: "postgres",
    host: "db.example.com",
    database: "northwind",
    user: "reader",
    secret: "connection:northwind",
  });
  expect(JSON.stringify(call[1].connection)).not.toContain("hunter2");
});

test("validates a server connection before writing its password", async () => {
  invoke.mockImplementation(async (command: string) => {
    if (command === "list_connections") return [];
    return undefined;
  });
  render(<ConnectionsDialog open onClose={() => {}} />);
  fireEvent.change(screen.getByLabelText("Name"), {
    target: { value: "broken" },
  });
  fireEvent.change(screen.getByLabelText("Kind"), {
    target: { value: "postgres" },
  });
  fireEvent.change(screen.getByLabelText("Password"), {
    target: { value: "hunter2" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Add connection" }));
  await waitFor(() =>
    expect(screen.getByRole("alert")).toHaveTextContent("needs a host"),
  );
  expect(invoke).not.toHaveBeenCalledWith("save_secret", expect.anything());
});

test("removes a new connection's password when the save fails", async () => {
  invoke.mockImplementation(async (command: string) => {
    if (command === "list_connections") return [];
    if (command === "save_secret") return "vault";
    if (command === "save_connection") throw new Error("name already exists");
    if (command === "delete_secret") return undefined;
    return undefined;
  });
  render(<ConnectionsDialog open onClose={() => {}} />);
  fireEvent.change(screen.getByLabelText("Name"), {
    target: { value: "northwind" },
  });
  fireEvent.change(screen.getByLabelText("Kind"), {
    target: { value: "postgres" },
  });
  fireEvent.change(screen.getByLabelText("Host"), {
    target: { value: "db.example.com" },
  });
  fireEvent.change(screen.getByLabelText("Database"), {
    target: { value: "northwind" },
  });
  fireEvent.change(screen.getByLabelText("User"), {
    target: { value: "reader" },
  });
  fireEvent.change(screen.getByLabelText("Password"), {
    target: { value: "hunter2" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Add connection" }));
  await waitFor(() =>
    expect(screen.getByRole("alert")).toHaveTextContent("already exists"),
  );
  expect(invoke).toHaveBeenCalledWith("delete_secret", {
    name: "connection:northwind",
    sessionOnly: false,
  });
});

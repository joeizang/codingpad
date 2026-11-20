import { createRef } from "react";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, test, vi } from "vitest";
import { connectionNameForFile, NewScriptMenu, SAMPLE_CONNECTION } from "./NewScriptMenu";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
const { open } = vi.hoisted(() => ({ open: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open }));

const shop = { id: "1", name: "shop", kind: "postgres" };

beforeEach(() => {
  invoke.mockReset();
  open.mockReset();
  invoke.mockImplementation(async (command: string) =>
    command === "list_connections" ? [shop] : undefined,
  );
});

function renderMenu(lastConnection?: string) {
  const anchor = createRef<HTMLButtonElement>();
  const onCreate = vi.fn();
  const onClose = vi.fn();
  render(
    <>
      <button ref={anchor}>+</button>
      <p>Outside</p>
      <NewScriptMenu
        open
        anchor={anchor}
        lastConnection={lastConnection}
        onCreate={onCreate}
        onClose={onClose}
      />
    </>,
  );
  return { onCreate, onClose };
}

test("Python is picked first, with no connection, and Enter creates it", async () => {
  const { onCreate } = renderMenu();
  await screen.findByRole("button", { name: /^shop/ });
  expect(screen.getByRole("radio", { name: "Python" })).toHaveFocus();
  expect(screen.getByRole("button", { name: "None" })).toHaveAttribute("aria-pressed", "true");
  fireEvent.keyDown(screen.getByRole("dialog"), { key: "Enter" });
  expect(onCreate).toHaveBeenCalledWith("python", undefined);
});

test("letter keys pick a language and SQL always has a connection", async () => {
  const { onCreate } = renderMenu("shop");
  await screen.findByRole("button", { name: /^shop/ });
  fireEvent.keyDown(screen.getByRole("dialog"), { key: "s" });
  expect(screen.getByRole("radio", { name: "SQL" })).toHaveAttribute("aria-checked", "true");
  expect(screen.getByRole("radio", { name: "SQL" })).toHaveFocus();
  expect(screen.getByRole("button", { name: "None" })).toBeDisabled();
  expect(screen.getByRole("button", { name: /^shop/ })).toHaveAttribute("aria-pressed", "true");
  fireEvent.click(screen.getByRole("button", { name: "Create SQL script on shop" }));
  expect(onCreate).toHaveBeenCalledWith("sql", "shop");
});

test("SQL falls back to the bundled sample database", async () => {
  invoke.mockResolvedValue([]);
  const { onCreate } = renderMenu();
  fireEvent.keyDown(screen.getByRole("dialog"), { key: "s" });
  fireEvent.keyDown(screen.getByRole("dialog"), { key: "Enter" });
  expect(onCreate).toHaveBeenCalledWith("sql", SAMPLE_CONNECTION);
});

test("Rust creates an unconnected script even after choosing a database", async () => {
  const { onCreate } = renderMenu();
  fireEvent.click(await screen.findByRole("button", { name: /^shop/ }));
  fireEvent.keyDown(screen.getByRole("dialog"), { key: "r" });
  expect(screen.queryByRole("group", { name: "Connection" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Create Rust script" }));
  expect(onCreate).toHaveBeenCalledWith("rust", undefined);
});

test.each([
  { name: "C#", key: "c", language: "csharp" },
  { name: "TypeScript", key: "t", language: "typescript" },
])("$name can attach an optional connection", async ({ name, key, language }) => {
  const { onCreate } = renderMenu();
  fireEvent.keyDown(screen.getByRole("dialog"), { key });
  fireEvent.click(await screen.findByRole("button", { name: /^shop/ }));
  fireEvent.click(screen.getByRole("button", { name: `Create ${name} script on shop` }));
  expect(onCreate).toHaveBeenCalledWith(language, "shop");
});

test("Escape and a click outside close it", async () => {
  const { onClose } = renderMenu();
  fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
  expect(onClose).toHaveBeenCalledTimes(1);
  fireEvent.mouseDown(screen.getByText("Outside"));
  expect(onClose).toHaveBeenCalledTimes(2);
  fireEvent.mouseDown(screen.getByRole("radio", { name: "C#" }));
  expect(onClose).toHaveBeenCalledTimes(2);
});

test("opening a database file saves it as a connection and picks it", async () => {
  open.mockResolvedValue("/data/Sales Q3.sqlite");
  invoke.mockImplementation(async (command: string, args?: { connection: { name: string } }) => {
    if (command === "list_connections") return [shop];
    if (command === "save_connection") return { ...args!.connection, id: "2" };
  });
  const { onCreate } = renderMenu();
  fireEvent.click(await screen.findByRole("button", { name: "Open a database file…" }));
  await waitFor(() =>
    expect(invoke).toHaveBeenCalledWith("save_connection", {
      connection: { id: "", name: "Sales-Q3", kind: "sqlite", path: "/data/Sales Q3.sqlite" },
    }),
  );
  fireEvent.click(await screen.findByRole("button", { name: "Create Python script on Sales-Q3" }));
  expect(onCreate).toHaveBeenCalledWith("python", "Sales-Q3");
});

test("database file names become unique connection names", () => {
  expect(connectionNameForFile("C:\\data\\shop.db", [])).toBe("shop");
  expect(connectionNameForFile("/x/shop.db", ["shop", "shop-2"])).toBe("shop-3");
  expect(connectionNameForFile("/x/codingpad-sample.sqlite", [])).toBe("codingpad-sample-2");
  expect(connectionNameForFile("/x/!!!.db", [])).toBe("database");
});

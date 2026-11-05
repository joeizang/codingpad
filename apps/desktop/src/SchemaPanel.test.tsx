import { act, fireEvent, render, renderHook, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { SchemaPanel } from "./SchemaPanel";
import { useConnectionSchema, type ConnectionSchema } from "./schema";
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const schema: ConnectionSchema = { kind: "sqlite", tables: [{ schema: "main", name: "orders", columns: [{ name: "user_id", dataType: "INTEGER", references: [{ schema: "main", table: "users", column: "id" }] }] }] };
describe("schema panel", () => {
  it("expands columns and relationships and refreshes on demand", () => {
    const refresh = vi.fn(); render(<SchemaPanel name="sample" schema={schema} loading={false} onRefresh={refresh} />);
    expect(screen.queryByText("user_id")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /main.orders/ }));
    expect(screen.getByText("INTEGER")).toBeInTheDocument(); expect(screen.getByText(/main.users.id/)).toBeInTheDocument();
    fireEvent.click(screen.getByText("Refresh schema")); expect(refresh).toHaveBeenCalledOnce();
  });
  it("bounds rendered rows for large schemas and filters beyond the first page", () => {
    const large = { ...schema, tables: Array.from({length: 5000}, (_, i) => ({ ...schema.tables[0], name: `table_${i}` })) };
    render(<SchemaPanel name="large" schema={large} loading={false} onRefresh={() => {}} />);
    expect(screen.getAllByRole("button", { name: /main.table_/ })).toHaveLength(100);
    fireEvent.change(screen.getByLabelText("Filter tables"), {target:{value:"table_4999"}});
    expect(screen.getByRole("button", { name: /main.table_4999/ })).toBeInTheDocument();
  });
  it("discards late responses after switching connections and retains snapshots on refresh failure", async () => {
    let resolve!: (value: ConnectionSchema) => void;
    vi.mocked(invoke).mockImplementationOnce(() => new Promise((done) => { resolve = done as typeof resolve; })).mockResolvedValueOnce(schema);
    const { result, rerender } = renderHook(({name, revision}) => useConnectionSchema(name, revision), {initialProps:{name:"old",revision:0}});
    rerender({name:"new",revision:0}); await waitFor(() => expect(result.current.schema).toBe(schema));
    await act(async () => resolve({ ...schema, tables: [] })); expect(result.current.schema).toBe(schema);
    vi.mocked(invoke).mockRejectedValueOnce("offline"); rerender({name:"new",revision:1});
    await waitFor(() => expect(result.current.error).toBe("offline")); expect(result.current.schema).toBe(schema);
    rerender({name:"",revision:1}); expect(result.current.schema).toBeUndefined();
  });
});

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { DumpTree } from "./DumpTree";
import type { DumpNode } from "./protocol";

function tableRows(from: number, count: number): DumpNode[][] {
  return Array.from({ length: count }, (_, offset) => [
    { kind: "string", v: `row ${from + offset}` },
    { kind: "primitive", v: from + offset },
  ]);
}

test("lazy nodes expand through the kernel", async () => {
  const expand = vi.fn(async () => ({ kind: "primitive", v: 7 }) as DumpNode);
  render(
    <DumpTree
      node={{
        kind: "lazy",
        type: "list",
        preview: "list (3 items)",
        handle: "h1",
      }}
      expand={expand}
    />,
  );
  fireEvent.click(screen.getByRole("button", { name: "Expand" }));
  expect(await screen.findByText("7")).toBeInTheDocument();
  expect(expand).toHaveBeenCalledWith("h1", null);
});

test("a 100k-row table mounts only visible rows and fetches pages on scroll", async () => {
  const expand = vi.fn(async (_handle: string, cursor: string | null) => ({
    kind: "table" as const,
    type: "list",
    columns: [{ name: "name" }, { name: "score" }],
    rows: tableRows(Number(cursor), 100),
    total: 100000,
    next: String(Number(cursor) + 100),
    handle: "h1",
  }));
  const { container } = render(
    <DumpTree
      node={{
        kind: "table",
        type: "list",
        columns: [{ name: "name" }, { name: "score" }],
        rows: tableRows(0, 100),
        total: 100000,
        next: "100",
        handle: "h1",
      }}
      expand={expand}
    />,
  );
  expect(container.querySelectorAll(".table-row")).toHaveLength(14);
  const viewport = container.querySelector(".table-viewport")!;
  Object.defineProperty(viewport, "scrollTop", {
    value: 27000,
    configurable: true,
  });
  fireEvent.scroll(viewport);
  expect(await screen.findByText(/row 1000/)).toBeInTheDocument();
  expect(expand).toHaveBeenCalledWith("h1", "1000");
  expect(container.querySelectorAll(".table-row").length).toBeLessThanOrEqual(14);
  expect(screen.queryByText(/row 99999/)).not.toBeInTheDocument();
});

test("collections load the next page without losing expanded state", async () => {
  const expand = vi.fn(async () => ({
    kind: "list" as const,
    type: "list",
    items: [{ kind: "primitive" as const, v: 2 }],
    total: 2,
    next: null,
    truncated: false,
  }));
  render(
    <DumpTree
      node={{
        kind: "list",
        type: "list",
        items: [{ kind: "primitive", v: 1 }],
        total: 2,
        next: "1",
        handle: "h2",
        truncated: true,
      }}
      expand={expand}
    />,
  );
  fireEvent.click(screen.getByRole("button", { name: "Show more" }));
  expect(await screen.findByText("2")).toBeInTheDocument();
  expect(expand).toHaveBeenCalledWith("h2", "1");
  expect(screen.queryByRole("button", { name: "Show more" })).toBeNull();
});

test("live updates replace the inner value and keep the badge", () => {
  const node: DumpNode = {
    id: "wrapper",
    kind: "live",
    node: { id: "inner", kind: "primitive", v: 1 },
  };
  const updates: Record<string, DumpNode> = {
    inner: { id: "inner-2", kind: "primitive", v: 2 },
  };
  render(<DumpTree node={node} updates={updates} />);
  expect(screen.getByText("2")).toBeInTheDocument();
  expect(screen.queryByText("1")).not.toBeInTheDocument();
  expect(screen.getByText("live")).toBeInTheDocument();
});

test("images, HTML and errors render with safe and clickable output", () => {
  const onOpenLine = vi.fn();
  const { container } = render(
    <>
      <DumpTree
        node={{
          kind: "image",
          mime: "image/png",
          src: "data:image/png;base64,AAAA",
          name: "plot.png",
        }}
      />
      <DumpTree node={{ kind: "html", html: "<b>trusted</b>" }} />
      <DumpTree
        node={{
          kind: "error",
          type: "ValueError",
          message: "boom",
          causes: [
            {
              type: "ZeroDivisionError",
              message: "division by zero",
            },
          ],
          traceback: [
            {
              file: "<codingpad>",
              line: 2,
              column: 0,
              function: "<module>",
              text: "raise ValueError('boom')",
              script: true,
            },
            {
              file: "/site-packages/lib.py",
              line: 9,
              column: 0,
              function: "helper",
              text: "",
              script: false,
            },
          ],
        }}
        onOpenLine={onOpenLine}
      />
    </>,
  );
  const image = container.querySelector("img")!;
  expect(image.getAttribute("src")).toBe("data:image/png;base64,AAAA");
  const frame = container.querySelector("iframe")!;
  expect(frame.getAttribute("sandbox")).toBe("");
  expect(frame.getAttribute("srcdoc")).toBe("<b>trusted</b>");
  fireEvent.click(screen.getByRole("button", { name: "line 2" }));
  expect(onOpenLine).toHaveBeenCalledWith(2);
  expect(container.querySelector(".dump-error")).toHaveTextContent("ValueError");
  expect(container.querySelector(".error-title")).toHaveTextContent("boom");
  expect(screen.getByText(/Caused by ZeroDivisionError/)).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "line 9" })).toBeNull();
});

test("a nested table cell expands below the table instead of clipping the row", async () => {
  const expand = vi.fn(async () => ({
    kind: "list" as const,
    type: "list",
    items: [{ kind: "primitive" as const, v: 9 }],
    total: 1,
    next: null,
    truncated: false,
  }));
  render(
    <DumpTree
      node={{
        kind: "table",
        type: "list",
        columns: [{ name: "value" }],
        rows: [
          [
            {
              kind: "lazy",
              type: "list",
              preview: "list (1 item)",
              handle: "cell",
            },
          ],
        ],
        total: 1,
        next: null,
      }}
      expand={expand}
    />,
  );
  fireEvent.click(screen.getByRole("button", { name: "Expand" }));
  expect(await screen.findByText("Expanded cell")).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Expand" }));
  await waitFor(() => expect(expand).toHaveBeenCalledTimes(2));
  expect(screen.getAllByText("Expanded cell")).toHaveLength(1);
  expect(screen.getByText("9")).toBeInTheDocument();
  expect(expand).toHaveBeenCalledWith("cell", null);
});

test("strings cut by a limit say they were truncated", () => {
  const { container } = render(
    <DumpTree node={{ kind: "string", v: "abc", truncated: true, total: 20000 }} />,
  );
  expect(container.textContent).toContain('"abc"');
  expect(container.textContent).toContain("truncated, 20000 chars");
});

test("unknown-length tables grow on scroll and stop at the last page", async () => {
  const expand = vi.fn(async () => ({
    kind: "table" as const,
    type: "IEnumerable",
    columns: [{ name: "name" }, { name: "score" }],
    rows: tableRows(100, 2),
    total: 102,
    totalKnown: true,
    next: null,
    handle: "seq",
  }));
  const { container } = render(
    <DumpTree
      node={{
        kind: "table",
        type: "IEnumerable",
        columns: [{ name: "name" }, { name: "score" }],
        rows: tableRows(0, 100),
        total: 101,
        totalKnown: false,
        next: "100",
        handle: "seq",
      }}
      expand={expand}
    />,
  );
  expect(screen.getByText("100+ rows")).toBeInTheDocument();
  const viewport = container.querySelector(".table-viewport")!;
  Object.defineProperty(viewport, "scrollTop", { value: 95 * 27, configurable: true });
  fireEvent.scroll(viewport);
  expect(await screen.findByText(/row 101/)).toBeInTheDocument();
  expect(screen.getByText("102 rows")).toBeInTheDocument();
  expect(expand).toHaveBeenCalledExactlyOnceWith("seq", "100");
  expect(container.querySelectorAll(".table-row").length).toBeLessThanOrEqual(14);
});

test("an endless table keeps growing one page at a time", async () => {
  const expand = vi.fn(async (_handle: string, cursor: string | null) => {
    const offset = Number(cursor);
    return {
      kind: "table" as const,
      type: "IEnumerable",
      columns: [{ name: "name" }, { name: "score" }],
      rows: tableRows(offset, 100),
      total: offset + 101,
      totalKnown: false,
      next: String(offset + 100),
      handle: "seq",
    };
  });
  const { container } = render(
    <DumpTree
      node={{
        kind: "table",
        type: "IEnumerable",
        columns: [{ name: "name" }, { name: "score" }],
        rows: tableRows(0, 100),
        total: 101,
        totalKnown: false,
        next: "100",
        handle: "seq",
      }}
      expand={expand}
    />,
  );
  const viewport = container.querySelector(".table-viewport")!;
  for (const offset of [100, 200]) {
    Object.defineProperty(viewport, "scrollTop", { value: (offset - 5) * 27, configurable: true });
    fireEvent.scroll(viewport);
    expect(await screen.findByText(`${offset + 100}+ rows`)).toBeInTheDocument();
    expect(expand).toHaveBeenLastCalledWith("seq", String(offset));
  }
  expect(expand).toHaveBeenCalledTimes(2);
});

test("truncated database results show a warning", () => {
  render(<DumpTree node={{ kind: "table", type: "DatabaseRows", columns: [{ name: "id" }], rows: [], total: 10000, truncated: true, next: null }} />);
  expect(screen.getByText(/results truncated/)).toBeInTheDocument();
});

const exportable: DumpNode = {
  kind: "table",
  type: "list",
  columns: [{ name: "name" }, { name: "score" }],
  rows: tableRows(0, 100),
  total: 100_000,
  next: "100",
  handle: "rows",
};

test("tables export every row with progress, and report the result", async () => {
  let report: (rows: number, total: number | null) => void = () => {};
  let finish: (summary: { rows: number; shortenedCells: number }) => void = () => {};
  const exportTable = vi.fn(
    (_handle: string, _format: string, onProgress: typeof report) =>
      new Promise<{ rows: number; shortenedCells: number }>((resolve) => {
        report = onProgress;
        finish = resolve;
      }),
  );
  render(<DumpTree node={exportable} exportTable={exportTable} />);
  fireEvent.click(screen.getByRole("button", { name: "Export Excel" }));
  expect(exportTable).toHaveBeenCalledWith("rows", "xlsx", expect.any(Function), expect.any(AbortSignal));
  expect(await screen.findByText("Exporting 0 of 100,000 rows…")).toBeInTheDocument();
  report(42_000, 100_000);
  expect(await screen.findByText("Exporting 42,000 of 100,000 rows…")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Export CSV" })).not.toBeInTheDocument();
  finish({ rows: 100_000, shortenedCells: 2 });
  expect(await screen.findByText(/Exported 100,000 rows; 2 long cells cut/)).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Export CSV" })).toBeInTheDocument();
});

test("cancelling an export aborts it and says so", async () => {
  const exportTable = vi.fn(
    (_handle: string, _format: string, _progress: unknown, signal: AbortSignal) =>
      new Promise<null>((_, reject) => signal.addEventListener("abort", () => reject(new Error("Export cancelled.")))),
  );
  render(<DumpTree node={exportable} exportTable={exportTable} />);
  fireEvent.click(screen.getByRole("button", { name: "Export CSV" }));
  fireEvent.click(await screen.findByRole("button", { name: "Cancel" }));
  expect(await screen.findByText("Export cancelled.")).toBeInTheDocument();
  expect(exportTable.mock.calls[0][3].aborted).toBe(true);
});

test("a failed export shows the error and dismissing the file dialog changes nothing", async () => {
  const exportTable = vi
    .fn()
    .mockResolvedValueOnce(null)
    .mockRejectedValueOnce("Excel supports at most 1048575 rows. Export this table as CSV instead.");
  render(<DumpTree node={exportable} exportTable={exportTable} />);
  fireEvent.click(screen.getByRole("button", { name: "Export Excel" }));
  await waitFor(() => expect(screen.getByRole("button", { name: "Export Excel" })).toBeInTheDocument());
  expect(screen.queryByText(/Exported/)).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Export Excel" }));
  expect(await screen.findByText(/at most 1048575 rows/)).toBeInTheDocument();
});

test("unknown totals show a running count and unmounting cancels the export", async () => {
  let signal: AbortSignal | undefined;
  const exportTable = vi.fn((_h: string, _f: string, _p: unknown, abort: AbortSignal) => {
    signal = abort;
    return new Promise<null>(() => {});
  });
  const { unmount } = render(
    <DumpTree node={{ ...exportable, totalKnown: false, total: 101 }} exportTable={exportTable} />,
  );
  fireEvent.click(screen.getByRole("button", { name: "Export CSV" }));
  expect(await screen.findByText("Exporting 0 rows…")).toBeInTheDocument();
  unmount();
  expect(signal?.aborted).toBe(true);
});

test("tables without an export callback show no export buttons", () => {
  render(<DumpTree node={exportable} />);
  expect(screen.queryByRole("button", { name: "Export CSV" })).not.toBeInTheDocument();
});

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

import { expect, test } from "vitest";
import {
  baseName,
  headerSummary,
  joinPath,
  parentPath,
  parseHeader,
} from "./library";

test("paths work with either separator", () => {
  expect(baseName("/library/hello.py")).toBe("hello.py");
  expect(baseName("C:\\library\\hello.py")).toBe("hello.py");
  expect(parentPath("/library/sub/hello.py")).toBe("/library/sub");
  expect(joinPath("/library", "hello.py")).toBe("/library/hello.py");
  expect(joinPath("/library/", "hello.py")).toBe("/library/hello.py");
  expect(joinPath("C:\\library", "hello.py")).toBe("C:\\library\\hello.py");
});

test("header settings render as a summary", () => {
  expect(
    headerSummary({
      line: "# codingpad: connection=northwind readOnly",
      settings: { connection: "northwind", readOnly: "" },
    }),
  ).toBe("connection=northwind · readOnly");
  expect(headerSummary(null)).toBe("");
});

test("parses the same header forms as the Rust store", () => {
  // Keep this example set in sync with crates/kernel-core/src/store.rs so the
  // Rust and TypeScript header parsers cannot drift.
  const examples: [
    string,
    { line: string; settings: Record<string, string> } | null,
  ][] = [
    [
      "# codingpad: connection=northwind\nprint(1)\n",
      {
        line: "# codingpad: connection=northwind",
        settings: { connection: "northwind" },
      },
    ],
    [
      "-- codingpad: connection=northwind\nSELECT 1\n",
      {
        line: "-- codingpad: connection=northwind",
        settings: { connection: "northwind" },
      },
    ],
    [
      '// codingpad: connection="north wind" theme=dark\n',
      {
        line: '// codingpad: connection="north wind" theme=dark',
        settings: { connection: "north wind", theme: "dark" },
      },
    ],
    ["\uFEFF  # codingpad: readOnly\r\n", {
      line: "  # codingpad: readOnly",
      settings: { readOnly: "" },
    }],
    [
      "  // codingpad: a=1  b='two words'\n",
      {
        line: "  // codingpad: a=1  b='two words'",
        settings: { a: "1", b: "two words" },
      },
    ],
    ["print(1)\n# codingpad: connection=one\n", null],
    ["# a normal comment\n", null],
    ["", null],
  ];
  for (const [source, expected] of examples) {
    expect(parseHeader(source)).toEqual(expected);
  }
});

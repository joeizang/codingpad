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
  expect(parseHeader("# codingpad: connection=northwind\nprint(1)")).toEqual({
    line: "# codingpad: connection=northwind",
    settings: { connection: "northwind" },
  });
  expect(parseHeader('// codingpad: connection="north wind" theme=dark')).toEqual({
    line: '// codingpad: connection="north wind" theme=dark',
    settings: { connection: "north wind", theme: "dark" },
  });
  expect(parseHeader("\uFEFF  # codingpad: readOnly")?.settings).toEqual({
    readOnly: "",
  });
  expect(parseHeader("print(1)\n# codingpad: connection=one")).toBeNull();
  expect(parseHeader("# a normal comment")).toBeNull();
});

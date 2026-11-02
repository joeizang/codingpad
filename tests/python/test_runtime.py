from concurrent.futures import ThreadPoolExecutor
import contextlib
import io
import json
import sys
import time
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "runtimes/python"))

import codingpad  # noqa: E402


class RuntimeTests(unittest.TestCase):
    def setUp(self):
        codingpad._begin(None, None)

    def tearDown(self):
        codingpad._end()

    def test_100k_row_table_produces_first_page_under_one_second(self):
        rows = [{"name": f"row {index}", "score": index} for index in range(100_000)]
        started = time.perf_counter()
        node = codingpad._tree(rows)
        elapsed = time.perf_counter() - started
        self.assertEqual(node["kind"], "table")
        self.assertEqual(node["total"], 100_000)
        self.assertEqual(len(node["rows"]), 100)
        self.assertEqual(node["next"], "100")
        self.assertLess(elapsed, 1.0)

    def test_table_pages_accept_an_arbitrary_cursor(self):
        rows = [{"score": index} for index in range(1000)]
        table = codingpad._tree(rows)
        page = codingpad._expand(table["handle"], "800", 100)
        self.assertEqual(len(page["rows"]), 100)
        self.assertEqual(page["rows"][0][0]["v"], 800)
        self.assertEqual(page["next"], "900")
        self.assertEqual(page["handle"], table["handle"])

    def test_a_wide_table_stays_within_the_page_byte_budget(self):
        rows = [
            {f"c{column}": "x" * 5000 for column in range(32)}
            for _ in range(100)
        ]
        node = codingpad._tree(rows)
        self.assertEqual(node["kind"], "table")
        self.assertLess(len(json.dumps(node)), 5_000_000)
        self.assertEqual(len(node["rows"][0][0]["v"]), 1000)

    def test_strings_past_the_budget_become_lazy_instead_of_empty(self):
        node = codingpad._tree([["x" * 10000] * 50 for _ in range(15)])
        strings = []
        lazy = []

        def walk(value):
            if value.get("kind") == "string":
                strings.append(value)
            if value.get("kind") == "lazy":
                lazy.append(value)
            for item in value.get("items", []):
                walk(item)
            for field in value.get("fields", []):
                walk(field["value"])

        walk(node)
        self.assertTrue(strings)
        self.assertTrue(all(value["v"] for value in strings))
        self.assertTrue(lazy)
        self.assertLess(len(json.dumps(node, ensure_ascii=True)), 6_000_000)

    def test_wire_budget_counts_escaped_unicode(self):
        node = codingpad._tree(["😀" * 10000] * 100)
        self.assertLess(len(json.dumps(node, ensure_ascii=True)), 6_000_000)

    def test_strings_cut_by_the_character_limit_are_marked(self):
        node = codingpad._tree("x" * 20000)
        self.assertEqual(len(node["v"]), 10000)
        self.assertTrue(node["truncated"])
        self.assertEqual(node["total"], 20000)
        table = codingpad._tree([{"value": "y" * 2000}])
        cell = table["rows"][0][0]
        self.assertEqual(len(cell["v"]), 1000)
        self.assertTrue(cell["truncated"])
        self.assertEqual(cell["total"], 2000)

    def test_cache_factory_can_wait_for_worker_using_another_key(self):
        outer, inner = object(), object()
        with ThreadPoolExecutor(max_workers=1) as pool:
            value = codingpad.cache(outer, lambda: pool.submit(
                codingpad.cache, inner, lambda: 42
            ).result(timeout=2))
        self.assertEqual(value, 42)
        self.assertEqual(codingpad.cache(outer, lambda: 99), 42)

    def test_dump_is_still_text_outside_the_app(self):
        self.assertIsNone(codingpad._sink)
        value = {"name": "Ada", "scores": [1, 2]}
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            self.assertIs(codingpad.dump(value), value)
            self.assertEqual(codingpad.dump(42), 42)
            codingpad.dump(codingpad.html("<b>hello</b>"))
        text = output.getvalue()
        self.assertIn("'name': 'Ada'", text)
        self.assertIn("42", text)
        self.assertIn("<b>hello</b>", text)


if __name__ == "__main__":
    unittest.main()

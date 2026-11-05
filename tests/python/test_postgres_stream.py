"""Optional PostgreSQL integration test (requires psycopg and a local server)."""
import os
import subprocess
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


@unittest.skipUnless(os.environ.get("CODINGPAD_TEST_PG_PORT"), "requires CODINGPAD_TEST_PG_* connection settings")
class PostgresStreamTests(unittest.TestCase):
    def test_large_result_is_bounded_and_connection_is_reusable(self):
        script = r'''
import os, resource, sys
sys.path.insert(0, "runtimes/python")
from codingpad import _Database, _tree
settings = {"kind": "postgres", "host": "127.0.0.1", "port": int(os.environ["CODINGPAD_TEST_PG_PORT"]),
            "database": os.environ["CODINGPAD_TEST_PG_DATABASE"], "user": os.environ["CODINGPAD_TEST_PG_USER"], "tls": "disable"}
db = _Database({"settings": settings, "password": os.environ["CODINGPAD_TEST_PG_PASSWORD"]})
before = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
rows = db.query("select n, repeat('x', 1000) as value from generate_series(1, 200000) n")
after = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
unit = 1 if sys.platform == "darwin" else 1024
assert (after - before) * unit < 128 * 1024 * 1024, (after - before) * unit
assert len(rows) == 10000 and rows.truncated and _tree(rows)["truncated"]
assert db.query("select %s as value", (42,))[0]["value"] == 42
assert db.query("select 1 as empty where false").columns == ["empty"]
assert db.query("create temporary table t(i int)").columns == []
assert db.query("insert into t select 1 where false returning i").columns == ["i"]
assert db.query("select count(*) as count from t")[0]["count"] == 0
inserted = db.query("insert into t select n from generate_series(1, 10001) n returning i")
assert len(inserted) == 10000 and inserted.truncated
assert db.query("select count(*) as count from t")[0]["count"] == 10001
db.close()
'''
        result = subprocess.run([sys.executable, "-c", script], cwd=ROOT, capture_output=True, text=True, timeout=60)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

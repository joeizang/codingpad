"""Keep the code-intelligence stub aligned with the public runtime API."""
import ast
from pathlib import Path
import unittest


class PublicStubTests(unittest.TestCase):
    def test_public_call_signatures_match(self):
        folder = Path(__file__).resolve().parents[2] / "runtimes/python/codingpad"

        def signatures(path):
            result = {}
            for node in ast.parse(path.read_text()).body:
                if isinstance(node, (ast.FunctionDef, ast.ClassDef)) and not node.name.startswith("_"):
                    if isinstance(node, ast.ClassDef):
                        methods = [method for method in node.body if isinstance(method, ast.FunctionDef)
                                   and (method.name == "__init__" or not method.name.startswith("_"))]
                    else:
                        methods = [node]
                    result[node.name] = [(method.name, [arg.arg for arg in method.args.args],
                                          len(method.args.defaults)) for method in methods]
            return result

        self.assertEqual(signatures(folder / "__init__.py"), signatures(folder / "__init__.pyi"))

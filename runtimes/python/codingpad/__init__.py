"""The built-in CodingPad helpers. No third-party dependencies."""

import math
from itertools import islice
from pprint import pprint

_sink = None
_MAX_ITEMS = 100
_MAX_DEPTH = 6
_MAX_NODES = 1000


def _tree(value, seen=None, depth=0, budget=None):
    """Bound eager dumps; avoid executing properties or consuming iterators."""
    budget = [_MAX_NODES] if budget is None else budget
    budget[0] -= 1
    if value is None:
        return {"kind": "null"}
    if isinstance(value, str):
        return {"kind": "string", "v": value[:10000]}
    if isinstance(value, (bool, int)):
        # Keep arbitrarily large Python integers out of JSON's number range.
        if isinstance(value, int) and abs(value) > 2**53 - 1:
            try:
                text = str(value)
            except ValueError:
                text = f"<int with {value.bit_length()} bits>"
            return {"kind": "string", "type": "int", "v": text[:10000]}
        return {"kind": "primitive", "v": value}
    if isinstance(value, float):
        return (
            {"kind": "primitive", "v": value}
            if math.isfinite(value)
            else {"kind": "string", "v": str(value)}
        )
    seen = set() if seen is None else seen
    if id(value) in seen:
        return {"kind": "string", "v": "<circular reference>"}
    if depth >= _MAX_DEPTH:
        return {"kind": "string", "v": "<maximum depth>"}
    seen.add(id(value))
    try:

        def child(item):
            return _tree(item, seen, depth + 1, budget)

        if isinstance(value, (list, tuple, set, frozenset)):
            items = []
            for item in islice(value, _MAX_ITEMS):
                if budget[0] <= 0:
                    break
                items.append(child(item))
            return {
                "kind": "list",
                "type": type(value).__name__,
                "items": items,
                "truncated": len(value) > len(items),
            }
        if isinstance(value, dict):
            attributes = value
        else:
            try:
                attributes = vars(value)
            except TypeError:
                return {
                    "kind": "string",
                    "type": type(value).__name__,
                    "v": repr(value)[:10000],
                }
        fields = []
        for name, item in islice(attributes.items(), _MAX_ITEMS):
            if budget[0] <= 0:
                break
            fields.append({"name": str(name)[:1000], "value": child(item)})
        return {
            "kind": "object",
            "type": type(value).__name__,
            "fields": fields,
            "truncated": len(attributes) > len(fields),
        }
    finally:
        seen.remove(id(value))


def dump(value):
    """Show a basic value tree and return the value for use in expressions."""
    if _sink is None:
        pprint(value)
    else:
        _sink(_tree(value))
    return value

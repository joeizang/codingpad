"""The built-in CodingPad helpers. No third-party dependencies."""

import base64
import math
import threading
import os
from itertools import islice
from pathlib import Path
from pprint import pprint

_sink = None
_update_sink = None
_registry = None

_MAX_ITEMS = 100
_MAX_TABLE_ROWS = 100
_MAX_TABLE_SAMPLE = 100
_MAX_COLUMNS = 32
_MAX_DEPTH = 6
_MAX_NODES = 1000
_MAX_STRING = 10000
_MAX_CELL_STRING = 1000
_MAX_PREVIEW = 200
_MAX_HTML = 200000
_MAX_IMAGE_BYTES = 4 * 1024 * 1024
_MAX_PAGE_BYTES = 4 * 1024 * 1024
_MAX_PAGE = 1000

_IMAGE_SIGNATURES = [
    (b"\x89PNG\r\n\x1a\n", "image/png"),
    (b"\xff\xd8\xff", "image/jpeg"),
    (b"GIF87a", "image/gif"),
    (b"GIF89a", "image/gif"),
    (b"BM", "image/bmp"),
]
_IMAGE_TYPES = {
    ".png": "image/png",
    ".jpg": "image/jpeg",
    ".jpeg": "image/jpeg",
    ".gif": "image/gif",
    ".webp": "image/webp",
    ".bmp": "image/bmp",
    ".svg": "image/svg+xml",
}


class _Registry:
    """Values kept alive for lazy and paged dumps until the next run."""

    def __init__(self):
        self.reset()

    def reset(self, run_id=None):
        self.run_id = run_id
        self.values = {}
        self.by_id = {}
        self.handles = 0
        self.nodes = 0

    def _name(self, prefix, number):
        return (
            f"{self.run_id}:{prefix}{number}"
            if self.run_id is not None
            else f"{prefix}{number}"
        )

    def handle(self, value):
        key = id(value)
        existing = self.by_id.get(key)
        if existing is not None:
            return existing
        self.handles += 1
        handle = self._name("h", self.handles)
        self.values[handle] = value
        self.by_id[key] = handle
        return handle

    def get(self, handle):
        if not isinstance(handle, str) or handle not in self.values:
            raise KeyError(handle)
        return self.values[handle]

    def node_id(self):
        self.nodes += 1
        return self._name("n", self.nodes)


_registry = _Registry()


class Html:
    """Markup that CodingPad renders in a sandboxed frame."""

    def __init__(self, markup):
        self.markup = str(markup)[:_MAX_HTML]

    def __repr__(self):
        return f"<CodingPad HTML: {len(self.markup)} characters>"


class Image:
    """An image file or byte string rendered in the results panel."""

    def __init__(self, source, mime=None):
        data, self.name = _read_image(source)
        if len(data) > _MAX_IMAGE_BYTES:
            raise ValueError(
                f"Image is {len(data)} bytes; the limit is {_MAX_IMAGE_BYTES} bytes"
            )
        self.mime = mime or _image_mime(data, self.name)
        self.data = data

    def __repr__(self):
        return f"<CodingPad image: {self.name or self.mime}, {len(self.data)} bytes>"


class Live:
    """A value whose dump updates whenever it is assigned."""

    def __init__(self, value=None):
        self._value = value
        self._targets = []

    @property
    def value(self):
        return self._value

    @value.setter
    def value(self, new_value):
        self._value = new_value
        self.update()

    def update(self):
        if _update_sink is None:
            return
        current = _registry.run_id
        self._targets = [target for target in self._targets if target[0] == current]
        for _, node_id in self._targets:
            _update_sink(node_id, _tree(self._value))


def _read_image(source):
    if isinstance(source, (bytes, bytearray, memoryview)):
        return bytes(source), None
    path = Path(os.fspath(source))
    return path.read_bytes(), path.name


def _image_mime(data, name):
    for signature, mime in _IMAGE_SIGNATURES:
        if data.startswith(signature):
            return mime
    if data.lstrip().startswith(b"<svg") or b"<svg" in data[:512]:
        return "image/svg+xml"
    if name:
        return _IMAGE_TYPES.get(Path(name).suffix.lower(), "application/octet-stream")
    return "application/octet-stream"


def html(markup):
    """Render trusted markup in a sandboxed results frame."""
    return Html(markup)


def image(source, mime=None):
    """Render a path, bytes or bytearray as an image."""
    return Image(source, mime)


def live(value=None):
    """Return a value whose dump updates when ``.value`` is assigned."""
    return Live(value)


def _begin(sink=None, update_sink=None, run_id=None):
    """Reset per-run state; called by the kernel before each script."""
    global _sink, _update_sink
    _sink = sink
    _update_sink = update_sink
    _registry.reset(run_id)


def _end():
    global _sink, _update_sink
    _sink = None
    _update_sink = None


def _node_id():
    return _registry.node_id()


def _make(kind, **fields):
    return {"id": _registry.node_id(), "kind": kind, **fields}


def _preview(value):
    name = type(value).__name__
    if isinstance(value, str):
        return f"{name} ({len(value)} chars)"[:_MAX_PREVIEW]
    if isinstance(value, dict):
        return f"{name} ({len(value)} fields)"[:_MAX_PREVIEW]
    if isinstance(value, (list, tuple, set, frozenset)):
        return f"{name} ({len(value)} items)"[:_MAX_PREVIEW]
    try:
        attributes = vars(value)
    except TypeError:
        return f"{name} object"[:_MAX_PREVIEW]
    return f"{name} ({len(attributes)} fields)"[:_MAX_PREVIEW]


def _lazy(value):
    return _make(
        "lazy",
        type=type(value).__name__,
        preview=_preview(value),
        handle=_registry.handle(value),
    )


def _wire_size(text):
    """Approximate the bytes after JSON's ensure_ascii escaping."""
    if text.isascii():
        return len(text)
    size = len(text)
    for character in text:
        codepoint = ord(character)
        if codepoint > 0xFFFF:
            size += 11
        elif codepoint > 0x7F:
            size += 5
    return size


def _string_node(text, budget, *, type=None, max_len=_MAX_STRING, value=None):
    trimmed = text[:max_len]
    cost = _wire_size(trimmed)
    if cost > budget[1]:
        # Keep the rest available: expanding starts with a fresh budget.
        return _lazy(text if value is None else value)
    budget[1] -= cost
    node = _make("string", v=trimmed)
    if type is not None:
        node["type"] = type
    if len(trimmed) < len(text):
        node["truncated"] = True
        node["total"] = len(text)
    return node


def _keys(record):
    if isinstance(record, dict):
        return list(record)
    try:
        return list(vars(record))
    except TypeError:
        return []


def _is_record(value):
    if isinstance(value, dict):
        return True
    try:
        vars(value)
    except TypeError:
        return False
    return True


def _is_table(value):
    if not isinstance(value, (list, tuple)) or not value:
        return False
    sample = value[:_MAX_TABLE_SAMPLE]
    return any(_keys(item) for item in sample) and all(
        _is_record(item) for item in sample
    )


def _columns(value):
    columns = {}
    for item in islice(value, _MAX_TABLE_SAMPLE):
        for key in _keys(item):
            if key not in columns:
                columns[key] = str(key)[:1000]
                if len(columns) >= _MAX_COLUMNS:
                    return list(columns.items())
    return list(columns.items())


def _cell(value, budget):
    if value is None or isinstance(value, (bool, int, float)):
        return _tree(value, budget=budget)
    if isinstance(value, str):
        return _string_node(value, budget, max_len=_MAX_CELL_STRING, value=value)
    return _lazy(value)


def _table(value, cursor, limit, budget):
    total = len(value)
    columns = _columns(value)
    row_limit = min(limit, _MAX_TABLE_ROWS)
    rows = []
    for item in islice(value, cursor, cursor + row_limit):
        if not _is_record(item):
            rows.append([_cell(item, budget)] + [_make("null") for _ in columns[1:]])
            continue
        record = item if isinstance(item, dict) else vars(item)
        rows.append([_cell(record.get(key), budget) for key, _ in columns])
    consumed = cursor + len(rows)
    next_cursor = str(consumed) if consumed < total else None
    node = _make(
        "table",
        type=type(value).__name__,
        columns=[{"name": label} for _, label in columns],
        rows=rows,
        total=total,
        next=next_cursor,
    )
    if next_cursor is not None:
        node["handle"] = _registry.handle(value)
    return node


def _sequence(value, depth, seen, budget, cursor, limit):
    total = len(value)
    page = list(islice(value, cursor, cursor + limit))
    items = [_tree(item, depth=depth + 1, seen=seen, budget=budget) for item in page]
    consumed = cursor + len(page)
    next_cursor = str(consumed) if consumed < total else None
    node = _make(
        "list",
        type=type(value).__name__,
        items=items,
        total=total,
        next=next_cursor,
        truncated=next_cursor is not None,
    )
    if next_cursor is not None:
        node["handle"] = _registry.handle(value)
    return node


def _object(value, attributes, depth, seen, budget, cursor, limit):
    items = list(attributes.items())
    page = items[cursor : cursor + limit]
    fields = [
        {
            "name": str(name)[:1000],
            "value": _tree(item, depth=depth + 1, seen=seen, budget=budget),
        }
        for name, item in page
    ]
    consumed = cursor + len(page)
    next_cursor = str(consumed) if consumed < len(items) else None
    node = _make(
        "object",
        type=type(value).__name__,
        fields=fields,
        total=len(items),
        next=next_cursor,
        truncated=next_cursor is not None,
    )
    if next_cursor is not None:
        node["handle"] = _registry.handle(value)
    return node


def _tree(value, *, depth=0, seen=None, budget=None, cursor=0, limit=_MAX_ITEMS):
    """Build a bounded dump node; deeper or larger values use lazy handles."""
    budget = [_MAX_NODES, _MAX_PAGE_BYTES] if budget is None else budget
    budget[0] -= 1
    if value is None:
        return _make("null")
    if isinstance(value, str):
        return _string_node(value, budget, value=value)
    if isinstance(value, bool):
        return _make("primitive", v=value)
    if isinstance(value, int):
        if abs(value) > 2**53 - 1:
            try:
                text = str(value)
            except ValueError:
                text = f"<int with {value.bit_length()} bits>"
            return _string_node(text, budget, type="int", value=value)
        return _make("primitive", v=value)
    if isinstance(value, float):
        return (
            _make("primitive", v=value)
            if math.isfinite(value)
            else _string_node(str(value), budget, value=value)
        )
    if isinstance(value, Html):
        cost = _wire_size(value.markup)
        if cost > budget[1]:
            return _lazy(value)
        budget[1] -= cost
        return _make("html", html=value.markup)
    if isinstance(value, Image):
        encoded = base64.b64encode(value.data).decode("ascii")
        source = f"data:{value.mime};base64,{encoded}"
        cost = _wire_size(source)
        if cost > budget[1]:
            return _lazy(value)
        budget[1] -= cost
        return _make("image", mime=value.mime, src=source, name=value.name)
    if isinstance(value, Live):
        wrapper_id = _registry.node_id()
        inner = _tree(value._value, depth=depth, seen=seen, budget=budget)
        value._targets.append((_registry.run_id, inner["id"]))
        return {"id": wrapper_id, "kind": "live", "node": inner}

    seen = set() if seen is None else seen
    if id(value) in seen:
        return _string_node("<circular reference>", budget)
    if depth >= _MAX_DEPTH or budget[0] <= 0 or budget[1] <= 0:
        return _lazy(value)

    seen.add(id(value))
    try:
        if isinstance(value, (list, tuple)):
            if _is_table(value):
                return _table(value, cursor, limit, budget)
            return _sequence(value, depth, seen, budget, cursor, limit)
        if isinstance(value, (set, frozenset)):
            return _sequence(value, depth, seen, budget, cursor, limit)
        if isinstance(value, dict):
            return _object(value, value, depth, seen, budget, cursor, limit)
        try:
            attributes = vars(value)
        except TypeError:
            return _string_node(repr(value), budget, type=type(value).__name__, value=value)
        return _object(value, attributes, depth, seen, budget, cursor, limit)
    finally:
        seen.remove(id(value))


def _expand(handle, cursor=None, limit=None):
    """Return a lazy value or another page of a container."""
    value = _registry.get(handle)
    try:
        offset = max(0, int(cursor)) if cursor is not None else 0
    except (TypeError, ValueError):
        raise ValueError("cursor must be a non-negative integer") from None
    try:
        size = int(limit) if limit is not None else _MAX_ITEMS
    except (TypeError, ValueError):
        raise ValueError("limit must be an integer") from None
    return _tree(value, cursor=offset, limit=max(1, min(size, _MAX_PAGE)))


def dump(value):
    """Show a rich value tree and return the value for use in expressions."""
    if _sink is None:
        if isinstance(value, Html):
            print(value.markup)
        elif isinstance(value, Image):
            print(value)
        else:
            pprint(value)
    else:
        _sink(_tree(value))
    return value


_cache = {}
_cache_lock = threading.RLock()


def cache(key, factory):
    """Compute a value once per key in this script's warm kernel.

    Failed computations are not cached. Reset clears every cached value.
    Concurrent misses may compute more than once; the first stored value wins.
    Factories run outside the lock so they can wait on workers using cache().
    """
    with _cache_lock:
        if key in _cache:
            return _cache[key]
    value = factory()
    with _cache_lock:
        return _cache.setdefault(key, value)

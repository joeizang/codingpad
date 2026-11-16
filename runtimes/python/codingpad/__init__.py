"""The built-in CodingPad helpers. No third-party dependencies."""

import base64
import json
from contextlib import closing
import math
import threading
import os
from itertools import islice
from pathlib import Path
from pprint import pprint

_sink = None
_update_sink = None
_registry = None
_secrets = {}
_secret_errors = {}

_MAX_ITEMS = 100
_MAX_TABLE_ROWS = 100
_MAX_DATABASE_ROWS = 10_000
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


class _DatabaseRows(list):
    def __init__(self, columns, rows, truncated=False):
        unique = []
        used = set()
        for name in columns:
            candidate = name
            suffix = 2
            while candidate in used:
                candidate = f"{name}_{suffix}"
                suffix += 1
            used.add(candidate)
            unique.append(candidate)
        super().__init__(dict(zip(unique, row)) for row in rows)
        self.columns = unique
        self.truncated = truncated


class _Database:
    """A connection belonging to one script run."""

    def __init__(self, config):
        self._config = config
        self._connection = None
        self._closed = False

    def _open(self):
        if self._closed:
            raise RuntimeError("This database client belongs to a finished script run.")
        if self._connection is not None:
            return self._connection
        if not self._config:
            raise RuntimeError("No connection is attached. Add a codingpad: connection=<name> header.")
        settings = self._config["settings"]
        kind = settings["kind"]
        if kind == "sqlite":
            import sqlite3
            connection = sqlite3.connect(settings["path"], isolation_level=None)
        elif kind == "postgres":
            import psycopg
            tls = settings.get("tls", "prefer")
            verified = tls != "disable" and not settings.get("trustServerCertificate")
            options = dict(host=settings["host"], port=settings.get("port") or 5432,
                dbname=settings["database"], user=settings["user"], password=self._config.get("password"),
                sslmode="verify-full" if verified else tls, connect_timeout=10, autocommit=True)
            if verified:
                options["sslrootcert"] = "system"
            try:
                connection = psycopg.connect(**options)
            except psycopg.OperationalError as error:
                # Prefer permits plaintext only when the server explicitly reports
                # that it does not support TLS; certificate failures never downgrade.
                if not (verified and tls == "prefer" and "server does not support SSL" in str(error)):
                    raise
                options["sslmode"] = "disable"
                options.pop("sslrootcert", None)
                connection = psycopg.connect(**options)
        elif kind == "sqlserver":
            import mssql_python
            def quote(value):
                return "{" + str(value or "").replace("}", "}}") + "}"
            tls = settings.get("tls", "prefer")
            encrypt = {"prefer": "Mandatory", "require": "Mandatory", "disable": "Optional"}[tls]
            connection = mssql_python.connect(";".join([
                "Server=" + quote(settings["host"] + "," + str(settings.get("port") or 1433)),
                "Database=" + quote(settings["database"]), "UID=" + quote(settings["user"]),
                "PWD=" + quote(self._config.get("password")), "Encrypt=" + encrypt,
                "TrustServerCertificate=" + ("yes" if settings.get("trustServerCertificate") else "no"),
            ]))
            connection.autocommit = True
        else:
            raise RuntimeError("Unsupported database type: " + str(kind))
        self._connection = connection
        return connection

    def query(self, sql, parameters=()):
        """Execute SQL and return rows as dictionaries suitable for dump()."""
        connection = self._open()
        cursor = connection.cursor()
        try:
            streamed = False
            if self._config["settings"]["kind"] == "postgres":
                import psycopg
                try:
                    # A regular psycopg cursor buffers the entire server result
                    # during execute, before fetchmany can enforce the cap.
                    with closing(cursor.stream(sql, parameters or None)) as stream:
                        rows = list(islice(stream, _MAX_DATABASE_ROWS + 1))
                        # Finishing the statement matters for INSERT/UPDATE ...
                        # RETURNING: cancelling at the cap can roll back writes.
                        # Discard later rows rather than keeping them in memory.
                        for _ in stream:
                            pass
                    streamed = True
                except psycopg.errors.SyntaxError as error:
                    # stream() uses the extended protocol, which rejects
                    # several statements in one string before running any of
                    # them. Fall back to the buffered path, which allows them.
                    if "multiple commands" not in str(error):
                        raise
                except psycopg.ProgrammingError as error:
                    # Statements without a result (DDL, SET). Covered by the
                    # live test in tests/python/test_postgres_stream.py.
                    if str(error) != "the operation in stream() didn't produce a result":
                        raise
                    return _DatabaseRows([], [])
            if streamed:
                if cursor.description is None:
                    # Psycopg's streaming API discards the metadata for zero
                    # rows. Describe an unnamed prepared statement without
                    # executing the user's SQL a second time.
                    with psycopg.ClientCursor(connection) as formatter:
                        query = formatter.mogrify(sql, parameters) if parameters else sql
                    with connection.lock:
                        prepared = connection.pgconn.prepare(b"", query.encode(connection.info.encoding))
                        if prepared.status != psycopg.pq.ExecStatus.COMMAND_OK:
                            raise psycopg.ProgrammingError(prepared.error_message.decode(connection.info.encoding))
                        described = connection.pgconn.describe_prepared(b"")
                        if described.status != psycopg.pq.ExecStatus.COMMAND_OK:
                            raise psycopg.ProgrammingError(described.error_message.decode(connection.info.encoding))
                        columns = [described.fname(i).decode(connection.info.encoding) for i in range(described.nfields)]
                else:
                    columns = [column[0] for column in cursor.description]
            else:
                cursor.execute(sql, parameters) if parameters else cursor.execute(sql)
                if cursor.description is None:
                    return _DatabaseRows([], [])
                columns = [column[0] for column in cursor.description]
                rows = cursor.fetchmany(_MAX_DATABASE_ROWS + 1)
            return _DatabaseRows(columns, rows[:_MAX_DATABASE_ROWS], len(rows) > _MAX_DATABASE_ROWS)
        finally:
            cursor.close()

    def execute(self, sql, parameters=()):
        """Execute a statement and return its affected row count."""
        connection = self._open()
        cursor = connection.cursor()
        try:
            cursor.execute(sql, parameters) if parameters else cursor.execute(sql)
            return cursor.rowcount
        finally:
            cursor.close()

    def close(self):
        connection, self._connection = self._connection, None
        self._config = None
        self._closed = True
        if connection is not None:
            connection.close()


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
        self.export_columns = {}

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


_database_session = None
_database_engine = None
_database_connection = None


def _set_database_connection(connection):
    global _database_connection
    _database_connection = dict(connection or {})


def _connect_data_context():
    if not _database_connection:
        raise RuntimeError(
            "No database connection is attached. Add one in Connections… and name it in the script header."
        )
    return _start_database(_database_connection)


def _start_database(connection):
    """Open the attached connection for generated SQLAlchemy models."""
    global _database_session, _database_engine
    try:
        from sqlalchemy import create_engine
        from sqlalchemy.engine import URL
        from sqlalchemy.orm import sessionmaker
    except ImportError as error:
        raise RuntimeError(
            "SQLAlchemy is not available in this Python environment. Run the script again; CodingPad prepares it for attached connections."
        ) from error
    kind = str(connection.get("kind") or "").lower()
    tls = str(connection.get("tls") or "prefer")
    trust = bool(connection.get("trustServerCertificate"))
    if kind == "sqlite":
        url = URL.create("sqlite", database=str(connection.get("path") or ""))
        engine = create_engine(url, future=True)
    elif kind == "postgres":
        # match the Rust and Level 2 paths: verify the certificate unless the
        # user explicitly trusts the server. `require` alone does not verify.
        sslmode = "disable" if tls == "disable" else ("verify-full" if not trust else tls)
        query = {"sslmode": sslmode}
        if sslmode == "verify-full":
            query["sslrootcert"] = "system"
        def postgres_url():
            return URL.create(
                "postgresql+psycopg",
                username=connection.get("user"),
                password=connection.get("password"),
                host=connection.get("host") or "localhost",
                port=connection.get("port") or 5432,
                database=connection.get("database"),
                query=query,
            )

        engine = create_engine(postgres_url(), future=True)
        if sslmode == "verify-full" and tls == "prefer":
            # Prefer permits plaintext only when the server explicitly reports
            # that it does not support TLS; certificate failures never downgrade.
            try:
                with engine.connect():
                    pass
            except Exception as error:
                if "server does not support SSL" not in str(error):
                    engine.dispose()
                    raise
                query["sslmode"] = "disable"
                query.pop("sslrootcert", None)
                engine.dispose()
                engine = create_engine(postgres_url(), future=True)
    elif kind == "sqlserver":
        try:
            import mssql_python
        except ImportError as error:
            raise RuntimeError(
                "The SQL Server driver is not available in this Python environment. Run the script again; CodingPad prepares it for attached connections."
            ) from error

        def quote(value):
            return "{" + str(value or "").replace("}", "}}") + "}"

        encrypt = "Optional" if tls == "disable" else "Mandatory"
        connection_string = ";".join(
            [
                "Server="
                + quote(
                    str(connection.get("host") or "localhost")
                    + ","
                    + str(connection.get("port") or 1433)
                ),
                "Database=" + quote(connection.get("database")),
                "UID=" + quote(connection.get("user")),
                "PWD=" + quote(connection.get("password")),
                "Encrypt=" + encrypt,
                "TrustServerCertificate=" + ("yes" if trust else "no"),
            ]
        )
        if not hasattr(mssql_python, "version"):
            mssql_python.version = getattr(mssql_python, "__version__", "1.0")
        engine = create_engine(
            "mssql+pyodbc://",
            module=mssql_python,
            creator=lambda: mssql_python.connect(connection_string),
            paramstyle="qmark",
            use_setinputsizes=False,
            future=True,
        )
        # pyodbc's optional output-converter hook is not available on this driver.
        engine.dialect.on_connect = lambda: None
    else:
        raise ValueError(f"Unsupported database kind: {kind or 'none'}")
    _database_engine = engine
    _database_session = sessionmaker(
        bind=_database_engine, expire_on_commit=False
    )()
    return _database_session


def _end():
    global _sink, _update_sink, _secrets, _secret_errors
    global _database_session, _database_engine, _database_connection
    if _database_session is not None:
        try:
            _database_session.close()
        except Exception:
            pass
        _database_session = None
    if _database_engine is not None:
        try:
            _database_engine.dispose()
        except Exception:
            pass
        _database_engine = None
    _database_connection = None
    _sink = None
    _update_sink = None
    _secrets = {}
    _secret_errors = {}


def _set_secrets(values, errors=None):
    global _secrets, _secret_errors
    _secrets = dict(values or {})
    _secret_errors = dict(errors or {})


def secret(name):
    """Return a named secret supplied by the OS password vault."""
    if not isinstance(name, str) or not name.strip():
        raise ValueError("A secret name cannot be empty")
    if name in _secret_errors:
        raise KeyError(_secret_errors[name])
    try:
        return _secrets[name]
    except KeyError:
        raise KeyError(
            f'Secret "{name}" was not found. Add it in Secrets… or set it for this session.'
        ) from None


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
        return [
            name for name in vars(record) if not str(name).startswith("_sa_")
        ]
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
    if isinstance(value, _DatabaseRows):
        return True
    if not isinstance(value, (list, tuple)) or not value:
        return False
    sample = value[:_MAX_TABLE_SAMPLE]
    return any(_keys(item) for item in sample) and all(
        _is_record(item) for item in sample
    )


def _columns(value):
    if isinstance(value, _DatabaseRows):
        return [(column, str(column)[:1000]) for column in value.columns[:_MAX_COLUMNS]]
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
    if isinstance(value, _DatabaseRows) and value.truncated:
        node["truncated"] = True
    # Always paged by handle so the whole table can be exported.
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
    if not isinstance(value, dict):
        attributes = {
            name: item
            for name, item in attributes.items()
            if not str(name).startswith("_sa_")
        }
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


def _export_cell(value):
    """A plain, untruncated cell for CSV/Excel export."""
    if value is None or isinstance(value, (bool, str)):
        return value
    if isinstance(value, int):
        return value if abs(value) <= 2**53 else {"n": str(value)}
    if isinstance(value, float):
        return value if math.isfinite(value) else str(value)
    if type(value).__name__ == "Decimal":
        return {"n": str(value)} if value.is_finite() else str(value)
    if isinstance(value, (bytes, bytearray, memoryview)):
        return "0x" + bytes(value).hex()
    isoformat = getattr(value, "isoformat", None)
    if callable(isoformat) and type(value).__module__ == "datetime":
        return isoformat()
    if isinstance(value, (list, tuple, dict)):
        try:
            return json.dumps(value, default=str, ensure_ascii=False)
        except (TypeError, ValueError):
            pass
    return str(value)


def _export_rows(handle, cursor=None, limit=None):
    """Return a page of a table dump as plain cells, for export."""
    value = _registry.get(handle)
    if not _is_table(value):
        raise ValueError("Only table dumps can be exported")
    try:
        offset = max(0, int(cursor)) if cursor is not None else 0
        size = max(1, min(int(limit) if limit is not None else _MAX_PAGE, _MAX_PAGE))
    except (TypeError, ValueError):
        raise ValueError("cursor and limit must be integers") from None
    columns = _registry.export_columns.get(handle)
    if columns is None:
        # Unlike the display, export every column of every row.
        if isinstance(value, _DatabaseRows):
            columns = list(value.columns)
        else:
            seen = {}
            for item in value:
                for key in _keys(item):
                    seen.setdefault(key, None)
            columns = list(seen)
        _registry.export_columns[handle] = columns
    rows = []
    for item in islice(value, offset, offset + size):
        fields = item if isinstance(item, dict) else vars(item) if _is_record(item) else {}
        rows.append([_export_cell(fields.get(key)) for key in columns])
    end = offset + len(rows)
    return {
        "columns": [str(column) for column in columns],
        "rows": rows,
        "next": str(end) if end < len(value) else None,
        "total": len(value),
    }


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

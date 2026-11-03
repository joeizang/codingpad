"""Public helper types for code intelligence, without analyzing renderer internals."""
from typing import Any, Callable, Hashable, TypeVar

_T = TypeVar("_T")

class Html:
    """Markup rendered in a sandboxed results frame."""
    markup: str
    def __init__(self, markup: object) -> None: ...

class Image:
    """An image file or byte string rendered in the results panel."""
    data: bytes
    name: str | None
    mime: str
    def __init__(self, source: Any, mime: str | None = None) -> None: ...

class Live:
    """A value whose dump updates whenever .value is assigned."""
    def __init__(self, value: Any = None) -> None: ...
    @property
    def value(self) -> Any: ...
    @value.setter
    def value(self, new_value: Any) -> None: ...
    def update(self) -> None: ...

def dump(value: _T) -> _T:
    """Show a rich value tree and return the value for use in expressions."""
    ...

def cache(key: Hashable, factory: Callable[[], _T]) -> _T:
    """Compute a value once per key in this script's warm kernel."""
    ...

def html(markup: object) -> Html:
    """Render trusted markup in a sandboxed results frame."""
    ...

def image(source: Any, mime: str | None = None) -> Image:
    """Render a path or byte string as an image."""
    ...

def live(value: Any = None) -> Live:
    """Return a value whose dump updates when .value is assigned."""
    ...

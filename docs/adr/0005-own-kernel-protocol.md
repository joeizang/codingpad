---
status: accepted
---

# CodingPad's own kernel protocol, not Jupyter's

The app talks to each language kernel over our own small JSON-RPC protocol, not the Jupyter messaging protocol. The protocol is designed around `dump()`: results stream as they are produced, objects are sent as lazy handles that expand on click, large tables are paged as the user scrolls, and runs can be cancelled cleanly. Jupyter's protocol only sends finished MIME bundles and cannot express any of that. We need custom kernels anyway (to provide `dump()`, `cache()` and the ready-connected `db`), so reusing existing Jupyter kernels would have saved little.

## Consequences
- We write and maintain a kernel for each of the four languages. Requests use stdin. Bun sends responses over an authenticated loopback connection because workers share the native stdout file descriptor. Python and C# send responses over stdout.
- A Jupyter adapter may be added later so existing Jupyter kernels (R, Julia, Go and others) can plug in with basic, non-lazy output.

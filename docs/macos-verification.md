# macOS verification — 3 November 2026

Acceptance runs for the M1 (#8) and M2 (#15) close gates, on an Apple M4 Max with
48 GB RAM, macOS 27.0.1 (26A434). The machine was in normal use, not quiesced.
Linux and Windows are not covered here.

## Environment

| Component | Version |
|---|---|
| Rust / Cargo | 1.96.0 |
| Node.js / npm | 26.7.0 / 11.19.0 |
| uv | 0.12.3 |
| just | 1.59.0 |
| .NET SDK / runtime | 10.0.402 / 10.0.13 |
| Python | 3.14.7 |

## Verdict

**M1 does not cleanly pass on macOS.** Four metrics and every functional check
pass. App usable and idle memory each miss their target intermittently.

**M2's C# targets pass on macOS** in every run, by a wide margin. `just bench`
reports M1 and M2 together, so its exit status can still fail on the M1 findings.

## Functional results

| Check | Result |
|---|---|
| `just check` | Passed, after the two test fixes below |
| Frontend tests | 58 passed |
| Python tests | 49 passed |
| C# kernel and runtime tests | 22 passed |
| Rust tests | 58 + 3 + 41 + 1 + 24 passed; 6 network tests ignored by default |
| Rust format, Clippy with warnings denied, Tauri debug build | Passed |
| Network acceptance: official uv and .NET installs, Python language server, PyPI package | 6 passed |
| Roslyn end-to-end C# code intelligence, with `CODINGPAD_ROSLYN_LSP` set | Passed |
| Terminal `dump()` fallback, run from plain `python3` outside CodingPad | Pretty-printed a dict, a list of dicts and a string |

## Speed results

Ten consecutive bench runs against one already-launched binary:

| Metric | Target | Min | Median | Max | Over target |
|---|---|---|---|---|---|
| App usable | < 1000 ms | 332.8 | 341.9 | 359.4 | 0/10 |
| Idle memory | < 300 MB | 183.8 | 186.0 | 426.0 | 1/10 |
| First Python kernel start | < 3000 ms | 33.0 | 36.0 | 39.8 | 0/10 |
| Warm Python rerun | < 200 ms | 0.4 | 0.4 | 0.6 | 0/10 |
| 100k-row dump, first page to host | < 1000 ms | 0.5 | 0.5 | 0.5 | 0/10 |
| Python completion | < 300 ms | 178.4 | 183.2 | 190.8 | 0/10 |
| First C# kernel start | < 3000 ms | 552.0 | 555.0 | 583.9 | 0/10 |
| Warm C# rerun after an edit | < 500 ms | 26.4 | 27.6 | 29.0 | 0/10 |
| 100k-item `IEnumerable`, first page to host | < 1000 ms | 148.1 | 150.2 | 157.6 | 0/10 |
| C# completion | < 300 ms | 17.4 | 18.4 | 21.0 | 0/10 |

These runs start from warm caches. The first Python kernel start in particular
does not include resolving or downloading anything. The C# metrics also passed
in both `just bench` runs that included them, which came straight after a build.

The bench's C# scripts declare no packages. D55 times reruns with packages
cached, so that was measured once separately: a script using a local-feed package
took 724.5 ms on its first run including restore, then 21.9 ms and 5.8 ms on two
edited reruns.

## Open findings

### App usable misses on the first launch after a build

Later launches take about 340 ms. The first launch of a newly built binary
crosses the 1 s target in three of five recorded samples:

| First launch after a build | App usable |
|---|---|
| `just bench` | 1178.8 ms (over) |
| `just bench` | 1076.3 ms (over) |
| `just bench` | 759.9 ms |
| Separate launch script | 1231 ms (over) |
| Separate launch script, after a 45 s settle | 965 ms |

Five launches of fresh copies of a built binary took 716–946 ms. Because
`just bench` rebuilds and relinks the app every time, it always measures this
cold launch, so the recipe can fail or pass from one run to the next. The cause
was not established. It could be first-launch work by macOS, the load left by
the build, or both, and a real user's first launch may be similarly slow.

### Idle memory occasionally exceeds 300 MB

Four of about 118 launches measured 411–426 MB against a normal 183–187 MB. The
two spikes captured per process were the app's own WebKit GPU helper, at about
251 MB instead of about 25 MB, so this is not another app's helper being counted.

The trigger was not found. Six rebuild-and-launch cycles did not reproduce it,
and it also occurred away from a rebuild. It was never caught long enough to
tell whether the memory is released, so it is unknown whether this is a brief
startup peak or lasting idle memory.

## Test fixes found by verification

- The C# package fixture ran `dotnet pack` through the macOS temp folder's
  symlink, which drops `ProjectReference` dependencies from the package. Two
  tests could not pass on a Mac with any of the three SDKs tried. The fixture
  now resolves the path. This did not affect the kernel.
- The optional PyPI test compared `sys.executable`, which uv reports as
  `bin/python` on a fresh environment and `bin/python3` on a cached one. It
  failed on every cold uv cache even though the same environment was reused. It
  now compares `sys.prefix`.

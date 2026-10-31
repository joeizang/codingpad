---
status: accepted
---

# C# scripts compile in memory with Roslyn, not via `dotnet run`

The C# kernel compiles each script with Roslyn into an in-memory assembly and runs it inside the warm kernel process (a collectible `AssemblyLoadContext` per run), the way LINQPad does. Reruns take a fraction of a second. .NET 10's official `dotnet run script.cs` would go through MSBuild and start a new process on every run, which takes a second or more and defeats the warm-process model (D11). Scripts still use the official `#:package` directives, so they remain runnable with plain `dotnet run`.

## Consequences
- The kernel resolves and restores NuGet packages itself (via NuGet client libraries) instead of relying on MSBuild.
- We must track any new `#:` directives that .NET adds, to stay compatible with `dotnet run`.

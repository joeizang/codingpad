using System.Collections.Immutable;
using System.Reflection;
using System.Runtime.InteropServices;
using System.Runtime.Loader;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using NuGet.Commands;
using NuGet.Common;
using NuGet.Configuration;
using NuGet.Frameworks;
using NuGet.LibraryModel;
using NuGet.ProjectModel;
using NuGet.Protocol.Core.Types;
using NuGet.RuntimeModel;
using NuGet.Versioning;

internal sealed class ScriptDependencies : AssemblyLoadContext
{
    private static readonly Dictionary<string, ScriptDependencies> Contexts = new();
    private static readonly Dictionary<string, string> FrameworkPaths = ((string)AppContext.GetData("TRUSTED_PLATFORM_ASSEMBLIES")!)
        .Split(Path.PathSeparator)
        .Where(p => Path.GetDirectoryName(p) == Path.GetDirectoryName(typeof(object).Assembly.Location))
        .Append(typeof(CodingPad.DumpExtensions).Assembly.Location)
        .Distinct().ToDictionary(p => Path.GetFileNameWithoutExtension(p), StringComparer.OrdinalIgnoreCase);
    private static readonly Dictionary<string, ScriptDependencies> Restores = new();
    private readonly Assets assets;
    internal IEnumerable<string> CompilePaths => assets.Compile.Values;
    private ScriptDependencies(Assets assets) : base("CodingPad dependencies", isCollectible: false) => this.assets = assets;

    // Plain data while discovery/restore is in progress: failed restores and
    // cache hits must never allocate a non-collectible load context.
    private sealed class Assets
    {
        internal Dictionary<string, string> Managed { get; } = new(StringComparer.OrdinalIgnoreCase);
        internal List<string> Native { get; } = new();
        internal Dictionary<string, string> Resources { get; } = new(StringComparer.OrdinalIgnoreCase);
        internal Dictionary<string, string> Compile { get; } = new(StringComparer.OrdinalIgnoreCase);
        internal IEnumerable<string> Paths => Compile.Values.Concat(Managed.Values).Concat(Resources.Values).Concat(Native);
        internal string Key => JsonSerializer.Serialize(new {
            Compile = Compile.OrderBy(p => p.Key), Managed = Managed.OrderBy(p => p.Key),
            Resources = Resources.OrderBy(p => p.Key), Native = Native.Order(StringComparer.Ordinal)
        });
        internal void AddManaged(string path, bool compile)
        {
            var name = Path.GetFileNameWithoutExtension(path);
            if (PreferFramework(path)) return;
            if (!Managed.TryGetValue(name, out var current) || AssemblyName.GetAssemblyName(path).Version >= AssemblyName.GetAssemblyName(current).Version)
                Managed[name] = path;
            if (compile) AddCompile(path);
        }
        internal void AddCompile(string path)
        {
            if (PreferFramework(path)) return;
            var name = Path.GetFileNameWithoutExtension(path);
            if (!Compile.TryGetValue(name, out var current) || AssemblyName.GetAssemblyName(path).Version >= AssemblyName.GetAssemblyName(current).Version)
                Compile[name] = path;
        }
    }

    protected override Assembly? Load(AssemblyName name)
    {
        // Never bind scripts to private host assemblies such as Roslyn/NuGet.
        if (assets.Managed.TryGetValue(name.Name!, out var implementation)) return LoadFromAssemblyPath(implementation);
        if (FrameworkPaths.ContainsKey(name.Name!)) return Default.LoadFromAssemblyName(name);
        if (!string.IsNullOrEmpty(name.CultureName))
            return assets.Resources.TryGetValue(name.CultureName + "/" + name.Name, out var resource) ? LoadFromAssemblyPath(resource) : null;
        throw new FileNotFoundException("Script dependency was not restored: " + name.Name);
    }
    protected override IntPtr LoadUnmanagedDll(string name)
    {
        var path = assets.Native.FirstOrDefault(p => Path.GetFileName(p) == name || Path.GetFileName(p) == name + ".dll" ||
            Path.GetFileName(p) == "lib" + name + ".so" || Path.GetFileName(p) == "lib" + name + ".dylib");
        return path is null ? IntPtr.Zero : LoadUnmanagedDllFromPath(path);
    }
    internal AssemblyLoadContext CreateScriptContext() => new ScriptContext(this);
    private sealed class ScriptContext(ScriptDependencies dependencies) : AssemblyLoadContext("script", isCollectible: true)
    {
        protected override Assembly? Load(AssemblyName name) => dependencies.Resolve(name);
        protected override IntPtr LoadUnmanagedDll(string name) => dependencies.LoadUnmanagedDll(name);
    }
    internal Assembly Resolve(AssemblyName name) => LoadFromAssemblyName(name);

    internal static async Task<ScriptDependencies> Restore(ScriptDirectives directives, string directory, Action<string> progress, CancellationToken cancel)
    {
        cancel.ThrowIfCancellationRequested();
        var dependencies = new Assets();
        string? restoreKey = null;
        if (directives.Web)
        {
            var shared = Directory.GetParent(Path.GetDirectoryName(typeof(object).Assembly.Location)!)!.Parent!.FullName;
            var root = Path.Combine(shared, "Microsoft.AspNetCore.App");
            var version = Directory.Exists(root) ? Directory.GetDirectories(root)
                .Where(p => Version.TryParse(Path.GetFileName(p), out var v) && v.Major == Environment.Version.Major)
                .OrderByDescending(p => Version.Parse(Path.GetFileName(p))).FirstOrDefault() : null;
            if (version is null) throw new InvalidOperationException("Microsoft.NET.Sdk.Web requires the ASP.NET Core " + Environment.Version.Major + " runtime. Install the matching .NET SDK or ASP.NET Core runtime.");
            foreach (var path in Directory.GetFiles(version, "*.dll")) dependencies.AddManaged(path, compile: true);
        }
        if (directives.Packages.Count > 0)
        {
            var settings = Settings.LoadDefaultSettings(directory);
            var sources = new PackageSourceProvider(settings).LoadPackageSources().Where(s => s.IsEnabled).ToList();
            var packagesPath = SettingsUtility.GetGlobalPackagesFolder(settings);
            var framework = NuGetFramework.ParseFolder($"net{Environment.Version.Major}.0");
            var runtimeGraph = JsonRuntimeFormat.ReadRuntimeGraph(Path.Combine(AppContext.BaseDirectory, "PortableRuntimeIdentifierGraph.json"));
            var rid = SelectRuntimeIdentifier(runtimeGraph, RuntimeInformation.RuntimeIdentifier,
                OperatingSystem.IsWindows() ? "win" : OperatingSystem.IsMacOS() ? "osx" : "linux",
                RuntimeInformation.ProcessArchitecture, IsMusl());
            restoreKey = RestoreKey(directives, directory, settings, packagesPath, rid, dependencies);
            if (Restores.TryGetValue(restoreKey, out var cached) && cached.assets.Paths.All(File.Exists))
            {
                cancel.ThrowIfCancellationRequested();
                return cached;
            }
            var temporary = Path.Combine(Path.GetTempPath(), "codingpad-restore-" + Guid.NewGuid().ToString("N"));
            Directory.CreateDirectory(temporary);
            try
            {
                var projectPath = Path.Combine(temporary, "script.csproj");
                var target = new TargetFrameworkInformation {
                    FrameworkName = framework,
                    RuntimeIdentifierGraphPath = Path.Combine(AppContext.BaseDirectory, "PortableRuntimeIdentifierGraph.json"),
                    // CodingPad itself is supplied by the host, including its imports.
                    Dependencies = directives.Packages.Where(p => !p.Key.Equals("CodingPad", StringComparison.OrdinalIgnoreCase))
                        .Select(p => new LibraryDependency { LibraryRange = new LibraryRange(p.Key, VersionRange.Parse(p.Value), LibraryDependencyTarget.Package) })
                        .ToImmutableArray()
                };
                var spec = new PackageSpec([target])
                {
                    Name = "CodingPadScript", FilePath = projectPath,
                    RestoreMetadata = new ProjectRestoreMetadata
                    {
                        ProjectStyle = ProjectStyle.PackageReference, ProjectPath = projectPath,
                        ProjectUniqueName = projectPath, ProjectName = "CodingPadScript",
                        OutputPath = temporary, PackagesPath = packagesPath,
                        FallbackFolders = SettingsUtility.GetFallbackPackageFolders(settings).ToList(),
                        Sources = sources, ConfigFilePaths = settings.GetConfigFilePaths().ToList(),
                        OriginalTargetFrameworks = [framework.GetShortFolderName()],
                        TargetFrameworks = [new ProjectRestoreMetadataFrameworkInfo(framework)]
                    },
                    // Portable RID fallbacks, shipped by the kernel rather than
                    // depending on an SDK being installed on the user's machine.
                    RuntimeGraph = new RuntimeGraph([new RuntimeDescription(rid)])
                };
                var graph = new DependencyGraphSpec();
                graph.AddProject(spec);
                graph.AddRestore(projectPath);
                using var cache = new SourceCacheContext();
                var logger = new RestoreLogger(progress);
                var args = new RestoreArgs { Log = logger, CacheContext = cache };
                args.PreLoadedRequestProviders.Add(new DependencyGraphSpecRequestProvider(new RestoreCommandProvidersCache(), graph, settings));
                var summaries = await RestoreRunner.RunAsync(args, cancel);
                if (summaries.Any(s => !s.Success)) throw new InvalidOperationException("NuGet restore failed. " + string.Join("\n", logger.Errors));
                var assets = new LockFileFormat().Read(Path.Combine(temporary, "project.assets.json"));
                var resolved = assets.Targets.FirstOrDefault(t => t.RuntimeIdentifier == rid) ?? assets.Targets.First(t => string.IsNullOrEmpty(t.RuntimeIdentifier));
                var fallbacks = runtimeGraph.ExpandRuntime(rid).ToHashSet();
                foreach (var library in resolved.Libraries)
                {
                    var package = assets.Libraries.First(l => l.Name == library.Name && l.Version == library.Version);
                    var folder = assets.PackageFolders.Select(f => Path.Combine(f.Path, package.Path)).First(Directory.Exists);
                    string Full(string path) => Path.GetFullPath(Path.Combine(folder, path.Replace('/', Path.DirectorySeparatorChar)));
                    foreach (var item in library.CompileTimeAssemblies.Where(i => i.Path.EndsWith(".dll")))
                        dependencies.AddCompile(Full(item.Path));
                    foreach (var item in library.RuntimeAssemblies.Where(i => i.Path.EndsWith(".dll")))
                        dependencies.AddManaged(Full(item.Path), compile: false);
                    foreach (var item in library.ResourceAssemblies)
                    {
                        var full = Full(item.Path);
                        dependencies.Resources[Path.GetFileName(Path.GetDirectoryName(full)) + "/" + Path.GetFileNameWithoutExtension(full)] = full;
                    }
                    foreach (var item in library.NativeLibraries.Where(i => !i.Path.EndsWith("_._"))) dependencies.Native.Add(Full(item.Path));
                    foreach (var group in library.RuntimeTargets.Where(i => fallbacks.Contains(i.Runtime)).GroupBy(i => i.AssetType))
                    {
                        var best = runtimeGraph.ExpandRuntime(rid).First(r => group.Any(i => i.Runtime == r));
                        foreach (var item in group.Where(i => i.Runtime == best && !i.Path.EndsWith("_._")))
                            if (item.AssetType == "native") dependencies.Native.Add(Full(item.Path));
                            else if (item.AssetType == "runtime" && item.Path.EndsWith(".dll")) dependencies.AddManaged(Full(item.Path), compile: false);
                    }
                }
            }
            finally { Directory.Delete(temporary, recursive: true); }
        }
        cancel.ThrowIfCancellationRequested();
        // Reuse package assembly identities so Cache can retain NuGet objects.
        var key = dependencies.Key;
        if (!Contexts.TryGetValue(key, out var context))
        {
            context = new ScriptDependencies(dependencies);
            Contexts.Add(key, context);
        }
        if (restoreKey is not null) Restores[restoreKey] = context;
        return context;
    }

    private static string RestoreKey(ScriptDirectives directives, string directory, ISettings settings, string packagesPath, string rid, Assets web)
    {
        // Read the current hierarchy every time: added configs, config edits,
        // environment substitutions and feed credentials must invalidate a hit.
        // Hash all inputs so secrets are never retained as dictionary keys.
        var input = JsonSerializer.Serialize(new {
            Directory = Path.GetFullPath(directory), directives.Web, Framework = Environment.Version.Major, Rid = rid,
            Packages = directives.Packages.OrderBy(p => p.Key, StringComparer.OrdinalIgnoreCase),
            PackagesPath = packagesPath, Fallbacks = SettingsUtility.GetFallbackPackageFolders(settings), WebAssets = web.Key,
            Configs = settings.GetConfigFilePaths().Select(path => new {
                Path = path, Content = Environment.ExpandEnvironmentVariables(File.ReadAllText(path))
            }),
            Credentials = Environment.GetEnvironmentVariables().Cast<System.Collections.DictionaryEntry>()
                .Where(entry => ((string)entry.Key).StartsWith("NuGetPackageSourceCredentials_", StringComparison.OrdinalIgnoreCase))
                .OrderBy(entry => (string)entry.Key, StringComparer.OrdinalIgnoreCase)
                .Select(entry => new { Name = (string)entry.Key, Value = (string?)entry.Value })
        });
        return Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(input)));
    }

    internal static string SelectRuntimeIdentifier(RuntimeGraph graph, string buildRid, string os, Architecture architecture, bool musl)
    {
        if (graph.Runtimes.ContainsKey(buildRid)) return buildRid;
        var portable = (os == "linux" && musl ? "linux-musl" : os) + "-" + architecture.ToString().ToLowerInvariant();
        if (!graph.Runtimes.ContainsKey(portable))
            throw new PlatformNotSupportedException("No supported NuGet runtime identifier for " + buildRid + " (" + portable + ").");
        return portable;
    }

    private static bool IsMusl()
    {
        if (!OperatingSystem.IsLinux()) return false;
        var rid = RuntimeInformation.RuntimeIdentifier;
        if (rid.Contains("musl", StringComparison.OrdinalIgnoreCase) || rid.StartsWith("alpine", StringComparison.OrdinalIgnoreCase)) return true;
        // Inspect the loaded libc, rather than any musl installation that might
        // happen to coexist with glibc on disk.
        try { return File.ReadLines("/proc/self/maps").Any(line => line.Contains("ld-musl-", StringComparison.Ordinal) || line.Contains("libc.musl-", StringComparison.Ordinal)); }
        catch (IOException) { return false; }
        catch (UnauthorizedAccessException) { return false; }
    }

    // Like the SDK's conflict resolution, retain platform assemblies when their
    // version is at least as new as a package's. Newer package implementations
    // remain isolated from the host (e.g. a newer System.Text.Json).
    private static bool PreferFramework(string path) =>
        FrameworkPaths.TryGetValue(Path.GetFileNameWithoutExtension(path), out var framework) &&
        AssemblyName.GetAssemblyName(framework).Version >= AssemblyName.GetAssemblyName(path).Version;

    private sealed class RestoreLogger(Action<string> progress) : LoggerBase
    {
        internal List<string> Errors { get; } = new();
        public override void Log(ILogMessage message)
        {
            if (message.Level == LogLevel.Error) Errors.Add(message.Code + ": " + message.Message);
            else if (message.Level == LogLevel.Warning) progress(message.Code + ": " + message.Message + "\n");
            else if (message.Level is LogLevel.Information or LogLevel.Minimal && message.Message.StartsWith("Installed ", StringComparison.Ordinal))
                progress(message.Message + "\n");
        }
        public override Task LogAsync(ILogMessage message) { Log(message); return Task.CompletedTask; }
    }
}

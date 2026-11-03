using System.Collections.Immutable;
using System.Reflection;
using System.Runtime.InteropServices;
using System.Runtime.Loader;
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
    private readonly Dictionary<string, string> managed = new(StringComparer.OrdinalIgnoreCase);
    private readonly List<string> native = new();
    private readonly Dictionary<string, string> resources = new(StringComparer.OrdinalIgnoreCase);
    private readonly Dictionary<string, string> compile = new(StringComparer.OrdinalIgnoreCase);
    internal IEnumerable<string> CompilePaths => compile.Values;
    private ScriptDependencies() : base("CodingPad dependencies", isCollectible: false) { }

    protected override Assembly? Load(AssemblyName name)
    {
        // Never bind scripts to private host assemblies such as Roslyn/NuGet.
        if (managed.TryGetValue(name.Name!, out var implementation)) return LoadFromAssemblyPath(implementation);
        if (FrameworkPaths.ContainsKey(name.Name!)) return Default.LoadFromAssemblyName(name);
        if (!string.IsNullOrEmpty(name.CultureName))
            return resources.TryGetValue(name.CultureName + "/" + name.Name, out var resource) ? LoadFromAssemblyPath(resource) : null;
        throw new FileNotFoundException("Script dependency was not restored: " + name.Name);
    }
    protected override IntPtr LoadUnmanagedDll(string name)
    {
        var path = native.FirstOrDefault(p => Path.GetFileName(p) == name || Path.GetFileName(p) == name + ".dll" ||
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
        var dependencies = new ScriptDependencies();
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
            var rid = RuntimeInformation.RuntimeIdentifier;
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
                    RuntimeGraph = NuGet.RuntimeModel.JsonRuntimeFormat.ReadRuntimeGraph(Path.Combine(AppContext.BaseDirectory, "PortableRuntimeIdentifierGraph.json"))
                };
                var runtimeGraph = spec.RuntimeGraph;
                spec.RuntimeGraph = new RuntimeGraph([new RuntimeDescription(rid)]);
                var graph = new DependencyGraphSpec();
                graph.AddProject(spec);
                graph.AddRestore(projectPath);
                using var cache = new SourceCacheContext();
                var logger = new RestoreLogger(progress);
                var args = new RestoreArgs { Log = logger, CacheContext = cache };
                args.PreLoadedRequestProviders.Add(new DependencyGraphSpecRequestProvider(new RestoreCommandProvidersCache(), graph, settings));
                progress("Restoring C# packages…\n");
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
                        dependencies.resources[Path.GetFileName(Path.GetDirectoryName(full)) + "/" + Path.GetFileNameWithoutExtension(full)] = full;
                    }
                    foreach (var item in library.NativeLibraries.Where(i => !i.Path.EndsWith("_._"))) dependencies.native.Add(Full(item.Path));
                    foreach (var group in library.RuntimeTargets.Where(i => fallbacks.Contains(i.Runtime)).GroupBy(i => i.AssetType))
                    {
                        var best = runtimeGraph.ExpandRuntime(rid).First(r => group.Any(i => i.Runtime == r));
                        foreach (var item in group.Where(i => i.Runtime == best && !i.Path.EndsWith("_._")))
                            if (item.AssetType == "native") dependencies.native.Add(Full(item.Path));
                            else if (item.AssetType == "runtime" && item.Path.EndsWith(".dll")) dependencies.AddManaged(Full(item.Path), compile: false);
                    }
                }
            }
            finally { Directory.Delete(temporary, recursive: true); }
        }
        // Reuse package assembly identities so Cache can retain NuGet objects.
        var key = string.Join('\n', dependencies.CompilePaths.Concat(dependencies.managed.Values).Concat(dependencies.resources.Values).Concat(dependencies.native).Order(StringComparer.Ordinal));
        if (Contexts.TryGetValue(key, out var existing)) return existing;
        Contexts.Add(key, dependencies);
        return dependencies;
    }

    private void AddManaged(string path, bool compile)
    {
        var name = Path.GetFileNameWithoutExtension(path);
        if (PreferFramework(path)) return;
        if (!managed.TryGetValue(name, out var current) || AssemblyName.GetAssemblyName(path).Version >= AssemblyName.GetAssemblyName(current).Version)
            managed[name] = path;
        if (compile) AddCompile(path);
    }

    // Like the SDK's conflict resolution, retain platform assemblies when their
    // version is at least as new as a package's. Newer package implementations
    // remain isolated from the host (e.g. a newer System.Text.Json).
    private static bool PreferFramework(string path) =>
        FrameworkPaths.TryGetValue(Path.GetFileNameWithoutExtension(path), out var framework) &&
        AssemblyName.GetAssemblyName(framework).Version >= AssemblyName.GetAssemblyName(path).Version;

    private void AddCompile(string path)
    {
        if (PreferFramework(path)) return;
        var name = Path.GetFileNameWithoutExtension(path);
        if (!compile.TryGetValue(name, out var current) || AssemblyName.GetAssemblyName(path).Version >= AssemblyName.GetAssemblyName(current).Version)
            compile[name] = path;
    }

    private sealed class RestoreLogger(Action<string> progress) : LoggerBase
    {
        internal List<string> Errors { get; } = new();
        public override void Log(ILogMessage message)
        {
            if (message.Level == LogLevel.Error) Errors.Add(message.Code + ": " + message.Message);
            if (message.Level >= LogLevel.Warning) progress(message.Code + ": " + message.Message + "\n");
        }
        public override Task LogAsync(ILogMessage message) { Log(message); return Task.CompletedTask; }
    }
}

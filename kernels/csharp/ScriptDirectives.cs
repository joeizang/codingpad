using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;
using NuGet.Versioning;

// One model for execution and the future shadow project (M2.4).
internal sealed class ScriptDirectives
{
    internal sealed class Invalid(string message, int line) : Exception(message)
    { internal int Line { get; } = line; }
    internal Dictionary<string, string> Packages { get; } = new(StringComparer.OrdinalIgnoreCase);
    internal Dictionary<string, string> Properties { get; } = new(StringComparer.OrdinalIgnoreCase);
    internal bool Web { get; private set; }

    internal static ScriptDirectives Parse(string source, CancellationToken cancel)
    {
        var result = new ScriptDirectives();
        var root = CSharpSyntaxTree.ParseText(source, cancellationToken: cancel).GetRoot(cancel);
        var first = root.GetFirstToken().SpanStart;
        var seen = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        foreach (var trivia in root.DescendantTrivia())
        {
            cancel.ThrowIfCancellationRequested();
            var text = trivia.ToString().Trim();
            if (!text.StartsWith("#:")) continue;
            var line = trivia.GetLocation().GetLineSpan().StartLinePosition.Line + 1;
            void Fail(string message) => throw new Invalid(message, line);
            if (trivia.SpanStart > first) Fail("File directives must appear before C# code.");
            var split = text[2..].IndexOfAny([' ', '\t']);
            if (split < 0) { Fail("Expected a directive name and value."); continue; }
            var kind = text.Substring(2, split);
            var value = text[(split + 3)..].Trim();
            if (kind == "project") Fail("#:project is not supported in CodingPad yet; runs with `dotnet run`.");
            if (kind == "package")
            {
                var parts = value.Split('@', 2, StringSplitOptions.TrimEntries);
                if (parts.Length != 2 || !VersionRange.TryParse(parts[1], out _) ||
                    parts[0].Length == 0 || parts[0].Any(c => !char.IsAsciiLetterOrDigit(c) && c is not ('.' or '-' or '_')))
                    Fail("Expected #:package Name@Version (NuGet version ranges and floating versions are supported).");
                if (!seen.Add("package:" + parts[0])) Fail("Duplicate package directive: " + parts[0]);
                result.Packages.Add(parts[0], parts[1]);
            }
            else if (kind == "sdk")
            {
                if (!seen.Add("sdk")) Fail("Only one SDK directive is supported.");
                if (value is not ("Microsoft.NET.Sdk" or "Microsoft.NET.Sdk.Web"))
                    Fail("Supported SDKs: Microsoft.NET.Sdk and Microsoft.NET.Sdk.Web.");
                result.Web = value == "Microsoft.NET.Sdk.Web";
            }
            else if (kind == "property")
            {
                var parts = value.Split('=', 2, StringSplitOptions.TrimEntries);
                if (parts.Length != 2 || parts[0].Length == 0) Fail("Expected #:property Name=Value.");
                if (!seen.Add("property:" + parts[0])) Fail("Duplicate property directive: " + parts[0]);
                var propertyValue = parts[1];
                if (propertyValue.StartsWith('"'))
                {
                    var expression = SyntaxFactory.ParseExpression(propertyValue);
                    if (expression is not LiteralExpressionSyntax literal || !literal.IsKind(SyntaxKind.StringLiteralExpression) || expression.ContainsDiagnostics)
                        Fail("Expected a quoted C# string value.");
                    propertyValue = ((LiteralExpressionSyntax)expression).Token.ValueText;
                }
                result.Properties.Add(parts[0], propertyValue);
                try { result.ValidateProperty(parts[0], propertyValue); }
                catch (ArgumentException e) { Fail(e.Message); }
            }
            else Fail("Unsupported file directive: #:" + kind);
        }
        return result;
    }

    private void ValidateProperty(string name, string value)
    {
        switch (name.ToLowerInvariant())
        {
            case "allowunsafeblocks": case "checkforoverflowunderflow": case "optimize":
            case "treatwarningsaserrors": case "codingpadimplicitusings":
                if (!bool.TryParse(value, out _)) throw new ArgumentException(name + " must be true or false.");
                break;
            case "nullable":
                if (value is not ("enable" or "disable" or "warnings" or "annotations")) throw new ArgumentException("Invalid Nullable value.");
                break;
            case "implicitusings":
                if (value is not ("enable" or "disable" or "true" or "false")) throw new ArgumentException("Invalid ImplicitUsings value.");
                break;
            case "langversion":
                if (!LanguageVersionFacts.TryParse(value, out _)) throw new ArgumentException("Invalid LangVersion; this feature may need a newer CodingPad.");
                break;
            case "defineconstants":
                if (value.Split([';', ','], StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries).Any(s => !SyntaxFacts.IsValidIdentifier(s)))
                    throw new ArgumentException("DefineConstants must contain C# identifiers separated by semicolons.");
                break;
            case "nowarn": case "warningsaserrors": case "warningsnotaserrors": break;
            case "warninglevel":
                if (!int.TryParse(value, out var level) || level < 0 || level > 9999) throw new ArgumentException("Invalid WarningLevel.");
                break;
            case "targetframework":
                if (value != $"net{Environment.Version.Major}.0") throw new ArgumentException("TargetFramework must match the selected runtime: net" + Environment.Version.Major + ".0.");
                break;
            // These deployment switches have no effect on in-memory execution.
            case "publishaot": case "packastool":
                if (!bool.TryParse(value, out _)) throw new ArgumentException(name + " must be true or false.");
                break;
            default: throw new ArgumentException("Unsupported compiler property: " + name);
        }
    }

    internal string Get(string name, string fallback) => Properties.GetValueOrDefault(name, fallback);
    internal bool Flag(string name, bool fallback = false) => bool.Parse(Get(name, fallback.ToString()));
    internal CSharpParseOptions ParseOptions => new CSharpParseOptions(
        LanguageVersionFacts.TryParse(Get("LangVersion", "default"), out var version) ? version : LanguageVersion.Default,
        preprocessorSymbols: new[] { "TRACE", "DEBUG", "NET", "NETCOREAPP", $"NET{Environment.Version.Major}_0" }
            .Concat(Enumerable.Range(5, Environment.Version.Major - 4).Select(major => $"NET{major}_0_OR_GREATER"))
            .Concat(new[] { "NETCOREAPP1_0_OR_GREATER", "NETCOREAPP1_1_OR_GREATER", "NETCOREAPP2_0_OR_GREATER", "NETCOREAPP2_1_OR_GREATER", "NETCOREAPP2_2_OR_GREATER", "NETCOREAPP3_0_OR_GREATER", "NETCOREAPP3_1_OR_GREATER" })
            .Concat(Get("DefineConstants", "").Split([';', ','], StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries)))
        .WithFeatures([new KeyValuePair<string, string>("FileBasedProgram", "true")]);

    internal CSharpCompilationOptions CompilationOptions
    {
        get
        {
            var diagnostics = new Dictionary<string, ReportDiagnostic>();
            foreach (var (name, mode) in new[] { ("WarningsAsErrors", ReportDiagnostic.Error), ("WarningsNotAsErrors", ReportDiagnostic.Warn), ("NoWarn", ReportDiagnostic.Suppress) })
                foreach (var code in Get(name, "").Split([';', ','], StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries))
                    diagnostics[code.All(char.IsDigit) ? "CS" + code.PadLeft(4, '0') : code] = mode;
            return new CSharpCompilationOptions(OutputKind.ConsoleApplication,
                optimizationLevel: Flag("Optimize") ? OptimizationLevel.Release : OptimizationLevel.Debug,
                checkOverflow: Flag("CheckForOverflowUnderflow"), allowUnsafe: Flag("AllowUnsafeBlocks"),
                nullableContextOptions: Enum.Parse<NullableContextOptions>(Get("Nullable", "enable"), true),
                warningLevel: int.Parse(Get("WarningLevel", "10")),
                generalDiagnosticOption: Flag("TreatWarningsAsErrors") ? ReportDiagnostic.Error : ReportDiagnostic.Default,
                specificDiagnosticOptions: diagnostics);
        }
    }

    internal SyntaxTree Usings
    {
        get
        {
            var names = new List<string>();
            if (Get("ImplicitUsings", "enable") is "enable" or "true")
            {
                names.AddRange(["System", "System.Collections.Generic", "System.IO", "System.Linq", "System.Net.Http", "System.Threading", "System.Threading.Tasks"]);
                if (Web) names.AddRange(["System.Net.Http.Json", "Microsoft.AspNetCore.Builder", "Microsoft.AspNetCore.Hosting", "Microsoft.AspNetCore.Http", "Microsoft.AspNetCore.Routing", "Microsoft.Extensions.Configuration", "Microsoft.Extensions.DependencyInjection", "Microsoft.Extensions.Hosting", "Microsoft.Extensions.Logging"]);
            }
            if (Flag("CodingPadImplicitUsings", true)) names.AddRange(["System", "System.Collections.Generic", "System.Linq", "System.Threading", "System.Threading.Tasks", "CodingPad", "static CodingPad.ScriptHelpers"]);
            return CSharpSyntaxTree.ParseText(string.Join("\n", names.Distinct().Select(name => "global using " + name + ";")), ParseOptions);
        }
    }
}

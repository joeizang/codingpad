# Publishing the name-reservation placeholders

These eight packages (version 0.0.1) reserve `codingpad` (the runtime library) and `cpad` (the CLI) on each registry (D37). The `cpad` placeholders install a `cpad` command that prints "not available yet" and exits 1.

Before publishing, create the GitHub repo `joeizang/codingpad`, so the homepage links in every package resolve.

## PyPI (needs a PyPI account and API token)
```sh
cd python/codingpad && uv build && uv publish   # prompts for / uses UV_PUBLISH_TOKEN
cd ../cpad       && uv build && uv publish
```

## npm (needs `npm login`)
```sh
cd npm/codingpad && npm publish
cd ../cpad       && npm publish
```

## crates.io (needs `cargo login` with a token and a verified email)
```sh
cd rust/codingpad && cargo publish
cd ../cpad        && cargo publish
```

## NuGet (needs an API key from nuget.org)
```sh
cd dotnet/codingpad && dotnet pack -c Release -o ../../dist
cd ../cpad          && dotnet pack -c Release -o ../../dist
dotnet nuget push ../../dist/*.nupkg --api-key <KEY> --source https://api.nuget.org/v3/index.json
```

These folders are throwaway. The real runtime libraries will live elsewhere in the monorepo and publish newer versions under the same names.

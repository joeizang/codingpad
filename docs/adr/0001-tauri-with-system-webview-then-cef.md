---
status: accepted
---

# Tauri 2 with system webviews now, bundled Chromium (CEF) later

CodingPad's desktop shell is Tauri 2: a Rust core with a web-based UI. We start on each OS's own webview (WebView2, WKWebView and WebKitGTK) because Tauri's bundled-Chromium runtime (`tauri-runtime-cef`) is still in alpha as of October 2026. We move to it once it is stable. We accept WebKitGTK's Linux issues (mainly blank or flickering windows on NVIDIA GPUs) for early releases, and mitigate them by detecting the problem and turning on a "safe drawing mode" (`WEBKIT_DISABLE_DMABUF_RENDERER=1`) automatically, with a setting so users can turn it on themselves. The UI avoids WebGL.

## Considered options
- **Electron**: mature bundled Chromium today, but a large download and a Node core. It remains the fallback if WebKitGTK problems prove unacceptable.
- **Tauri on the CEF alpha from day one**: identical rendering immediately, but we would be building on prerelease software (Linux GPU and Windows sandbox bugs open).
- **Avalonia**: native .NET UI, but rich HTML `dump()` output and the Monaco editor are much harder to build.

## Consequences
- Bundling CEF later will make the download roughly as large as an Electron app. At that point Tauri's remaining advantage is the Rust core.
- Linux testing must cover at least one NVIDIA machine and two distro versions of WebKitGTK.

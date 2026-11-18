---
status: accepted
---

# Unsigned releases and no auto-updater, for budget reasons

Early releases are distributed through GitHub Releases without paid OS code signing, and without an auto-updater. The app only checks GitHub Releases and links to the release page when a new version exists. This is purely a budget constraint, not a technical preference: Apple's Developer Program costs $99/year, and Windows signing certificates cost money too.

## Consequences
- macOS users must use "Open Anyway" in System Settings on first launch, and Windows users must get past the SmartScreen "Run anyway" screen. The README and download page carry step-by-step first-launch instructions with screenshots.
- Homebrew's main cask repo does not accept unsigned apps, so macOS distribution via Homebrew uses our own tap.
- Next steps when possible: apply to SignPath Foundation for free Windows signing for OSS; buy Apple signing once there is traction or sponsorship; turn on Tauri's built-in updater (its update signing key is free).

## Note (2026-11-18)

The 0.1 preview ships text-only first-launch instructions in the README: the
Windows SmartScreen steps now, since macOS installers are deferred by D62.
Screenshots are added to the release page once the builds exist.

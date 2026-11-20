#!/usr/bin/env bash
set -euo pipefail

if [[ $# != 2 || ! $1 =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "Usage: $0 vMAJOR.MINOR.PATCH INSTALLERS_DIR" >&2
  exit 1
fi

version=${1#v}
installers=$2
arm_checksum=$(shasum -a 256 < "$installers/CodingPad_${version}_aarch64.dmg")
intel_checksum=$(shasum -a 256 < "$installers/CodingPad_${version}_x64.dmg")

cat <<EOF
cask "codingpad" do
  arch arm: "aarch64", intel: "x64"

  version "$version"
  sha256 arm:   "${arm_checksum%% *}",
         intel: "${intel_checksum%% *}"

  url "https://github.com/joeizang/codingpad/releases/download/v#{version}/CodingPad_#{version}_#{arch}.dmg"
  name "CodingPad"
  desc "Desktop scratchpad for running scripts and inspecting their results"
  homepage "https://github.com/joeizang/codingpad"

  livecheck do
    url :url
    strategy :github_latest
  end

  depends_on :macos

  app "CodingPad.app"

  zap trash: [
    "~/Library/Application Support/io.github.joeizang.codingpad",
    "~/Library/Caches/codingpad",
    "~/Library/Caches/io.github.joeizang.codingpad",
    "~/Library/Preferences/io.github.joeizang.codingpad.plist",
    "~/Library/Saved Application State/io.github.joeizang.codingpad.savedState",
    "~/Library/WebKit/io.github.joeizang.codingpad",
  ]
end
EOF

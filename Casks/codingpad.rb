cask "codingpad" do
  arch arm: "aarch64", intel: "x64"

  version "0.3.1"
  sha256 arm:   "698ad517a6ccf4e5010b0836bba765a922c94e09ac414e8c1ef15d8a79bbbf4e",
         intel: "7d0b6bcd6a8c5fcf13364028e0ee701010014ce4a4001e9d50cc7a7006e96c04"

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

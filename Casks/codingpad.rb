cask "codingpad" do
  arch arm: "aarch64", intel: "x64"

  version "0.2.0"
  sha256 arm:   "d0d4a9b610be4c426653828d6964495edb5687a779572d964108e6587f0e7a19",
         intel: "f62fb8b5a80374274b986181eb3802498e439b92d983c649366accb789ba6f04"

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

cask "codingpad" do
  arch arm: "aarch64", intel: "x64"

  version "0.1.0"
  sha256 arm:   "e195a23a5a0f8f41049fb9d81029b4c9e088080a933b3235185dbc114aebc7e4",
         intel: "8b7b06750d5f231345a83a4cc3d5ecfbe1e3de0ae14b37775bcf0b5af0cbc2d5"

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

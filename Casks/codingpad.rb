cask "codingpad" do
  arch arm: "aarch64", intel: "x64"

  version "0.3.0"
  sha256 arm:   "615b42461e60c5bd5fa40072ad7126d77e2ed202934b33ca311595a8c22a245a",
         intel: "baf43406356f9cbc04850089d0d37692ead1a24817f80dee4c4a157b02cfa890"

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

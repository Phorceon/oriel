cask "oriel" do
  version "0.1.0"
  sha256 "eb45cc9779b4e29f897e326502fa2b7fc2af3dfdc2122e4d5607b1134d14b16f"

  url "https://github.com/Phorceon/homebrew-tap/releases/download/oriel-v#{version}/Oriel-#{version}.zip"
  name "Oriel"
  desc "Chrome-style tabs for real desktop app windows"
  homepage "https://github.com/Phorceon/homebrew-tap"

  depends_on arch: :arm64
  depends_on macos: ">= :ventura"

  app "Oriel.app"

  zap trash: "~/Library/Application Support/Oriel"

  caveats <<~EOS
    Oriel is not notarized by Apple. On first open, macOS blocks it.
    Allow it in System Settings > Privacy & Security ("Open Anyway").

    Oriel needs the Accessibility permission to move other apps' windows.
    Grant it in System Settings > Privacy & Security > Accessibility.
  EOS
end

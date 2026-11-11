cask "oriel" do
  version "0.1.0"
  sha256 "0ec97469efa9cc7993873892ad1b2e3ac1e3a41167a873df6368768123752810"

  url "https://github.com/Phorceon/oriel/releases/download/v#{version}/Oriel-#{version}.zip"
  name "Oriel"
  desc "Tab bar for real desktop app windows"
  homepage "https://github.com/Phorceon/oriel"

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

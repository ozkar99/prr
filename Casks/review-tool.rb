# Homebrew cask for Review Tool, built by CI (.github/workflows/release.yml) and
# published as a GitHub Release.
#
#   brew tap ozkar99/review-tool https://github.com/ozkar99/review-tool
#   brew trust --cask ozkar99/review-tool/review-tool
#   brew install --cask review-tool
cask "review-tool" do
  version "0.3.0"
  sha256 "1f41641aadaef4a0898d758cf2f74761743933c605a3d87083610ab979c2342c"

  url "https://github.com/ozkar99/review-tool/releases/download/v#{version}/Review.Tool_#{version}_universal.dmg"
  name "Review Tool"
  desc "Review GitHub pull requests with real code intelligence"
  homepage "https://github.com/ozkar99/review-tool"

  depends_on formula: "gh"   # GitHub access; the app reuses its login
  depends_on formula: "node" # runs pyright and bash-language-server (Python and shell support)

  app "Review Tool.app"
  # The app's binary is also the `review-tool` command.
  binary "#{appdir}/Review Tool.app/Contents/MacOS/review-tool"

  # The app is ad-hoc signed, not notarized, so macOS would block it as a
  # downloaded app. Installing from this tap is the trust decision (see
  # `brew trust`), so clear the download quarantine flag.
  postflight_steps do
    run "/usr/bin/xattr",
        args:           ["-dr", "com.apple.quarantine", "#{appdir}/Review Tool.app"],
        writable_paths: ["Review Tool.app"],
        writable_base:  :appdir
  end

  zap trash: [
    "~/Library/Application Support/review-tool",
    "~/Library/Caches/com.ozkar99.review-tool",
    "~/Library/WebKit/com.ozkar99.review-tool",
  ]

  caveats <<~EOS
    Log in to GitHub if you haven't (the app uses gh's login):
      gh auth login
    Open a PR with:
      review-tool owner/repo#123
  EOS
end

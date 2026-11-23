# Homebrew cask for prr (pronounced "purr"), built by CI
# (.github/workflows/release.yml) and published as a GitHub Release.
#
#   brew tap ozkar99/prr https://github.com/ozkar99/prr
#   brew trust --cask ozkar99/prr/prr
#   brew install --cask prr
cask "prr" do
  version "4.1.3"
  sha256 "4b9bb60253db0eae1d33cfb3a9b0b13bdf4237e91a55f19aa4afdc6537475839"

  url "https://github.com/ozkar99/prr/releases/download/v#{version}/prr_#{version}_universal.dmg"
  name "prr"
  desc "Review GitHub pull requests with real code intelligence"
  homepage "https://github.com/ozkar99/prr"

  depends_on formula: "node" # runs pyright and bash-language-server (Python and shell support)

  app "prr.app"
  # The app's binary is also the `prr` command.
  binary "#{appdir}/prr.app/Contents/MacOS/prr"

  # The app is ad-hoc signed, not notarized, so macOS would block it as a
  # downloaded app. Installing from this tap is the trust decision (see
  # `brew trust`), so clear the download quarantine flag.
  postflight_steps do
    run "/usr/bin/xattr",
        args:           ["-dr", "com.apple.quarantine", "{{appdir}}/prr.app"],
        writable_paths: ["prr.app"],
        writable_base:  :appdir
  end

  # The app keeps its data where earlier versions did, so settings and checkouts carry over.
  zap trash: [
    "~/Library/Application Support/review-tool",
    "~/Library/Caches/com.ozkar99.review-tool",
  ]

  caveats <<~EOS
    prr finds a GitHub token by itself (git's credentials for
    https://github.com, $GH_TOKEN, or gh's login). Without one, add a
    token in prr → Settings (⌘,).
    Open a PR with:
      prr owner/repo#123
  EOS
end

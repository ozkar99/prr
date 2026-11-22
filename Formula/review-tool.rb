# Homebrew formula that builds Review Tool on the user's Mac. Building locally
# means macOS never quarantines the app, so it opens without Gatekeeper prompts
# and no Apple Developer signing is needed.
#
#   brew tap ozkar99/review-tool https://github.com/ozkar99/review-tool
#   brew install review-tool
class ReviewTool < Formula
  desc "Review GitHub pull requests with real code intelligence"
  homepage "https://github.com/ozkar99/review-tool"
  url "https://github.com/ozkar99/review-tool/archive/refs/tags/v0.1.0.tar.gz"
  sha256 "8e8c89bd6992b39adcce6d22881b26ef88386bea792269bc212f84405372830f"
  head "https://github.com/ozkar99/review-tool.git", branch: "main"

  depends_on "rust" => :build
  depends_on :macos
  depends_on "gh"   # GitHub access; the app reuses its login
  depends_on "node" # builds the frontend; runs pyright for Python support

  def install
    system "npm", "ci", "--no-audit", "--no-fund"
    system "npm", "run", "build"
    prefix.install "src-tauri/target/release/bundle/macos/Review Tool.app"
    # The app's binary is also the `review-tool` command.
    bin.install_symlink prefix/"Review Tool.app/Contents/MacOS/review-tool"
  end

  def caveats
    <<~EOS
      To open Review Tool from Launchpad or Spotlight, link it into Applications:
        ln -sf "#{opt_prefix}/Review Tool.app" ~/Applications/

      Then log in to GitHub if you haven't: gh auth login
      Open a PR with: review-tool owner/repo#123
    EOS
  end

  test do
    assert_match "Usage: review-tool", shell_output("#{bin}/review-tool --help")
  end
end

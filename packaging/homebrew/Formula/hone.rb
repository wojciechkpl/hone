class Hone < Formula
  desc "Rust-native AI agent runtime — TUI, web UI, and orchestration"
  homepage "https://wojciechkpl.github.io/hone/"
  license "MIT OR Apache-2.0"
  head "https://github.com/wojciechkpl/hone.git", branch: "main"

  # When a tagged release exists, switch to the URL+SHA pattern. Until then,
  # users install via `brew install --HEAD wojciechkpl/hone/hone`.
  #
  #   url "https://github.com/wojciechkpl/hone/archive/refs/tags/v0.1.0.tar.gz"
  #   sha256 "<filled by `brew create` or `brew bump`>"
  #   version "0.1.0"

  depends_on "rust" => :build
  depends_on "node" => :build  # only required when building the embedded web UI

  # Linux runtime deps for the optional Tauri desktop shell. The CLI/daemon
  # don't need them but they are cheap and ensure parity with the desktop
  # app build path documented in docs/getting-started.md § 2.
  on_linux do
    depends_on "pkg-config" => :build
    depends_on "openssl"
    depends_on "webkit2gtk" => :recommended
  end

  def install
    # Build the embedded SolidJS web UI first so rust-embed picks it up
    # when honed compiles. Skip silently if Node is unavailable; the daemon
    # falls back to a stub page in that case (build.rs writes one).
    if which("npm")
      cd "apps/webui" do
        system "npm", "install", "--no-audit", "--no-fund"
        system "npm", "run", "build"
      end
    else
      ohai "npm not found — skipping web UI build (honed will serve a stub at /ui)"
    end

    # Build both binaries: hone (CLI/TUI) and honed (HTTP daemon).
    system "cargo", "install", *std_cargo_args(path: "bins/hone")
    system "cargo", "install", *std_cargo_args(path: "bins/honed")
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/hone --version")
    assert_match version.to_s, shell_output("#{bin}/honed --version")
  end
end

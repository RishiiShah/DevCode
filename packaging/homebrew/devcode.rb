# Homebrew formula template. It lives in a tap repository, not here: create
# github.com/RishiiShah/homebrew-devcode and copy this file to Formula/devcode.rb.
# HEAD installs work immediately; stable installs need url and sha256 for a tagged release.
class Devcode < Formula
  desc "Coding-agent harness: minimal, verified, reversible changes"
  homepage "https://github.com/RishiiShah/DevCode"
  head "https://github.com/RishiiShah/DevCode.git", branch: "main"
  # url "https://github.com/RishiiShah/DevCode/archive/refs/tags/vVERSION.tar.gz"
  # sha256 "SHA256_OF_THAT_TARBALL"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args
  end

  test do
    assert_match "devcode", shell_output("#{bin}/devcode --version")
  end
end

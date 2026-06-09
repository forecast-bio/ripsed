# Homebrew formula template — copy into a homebrew-ripsed tap's Formula/
# directory and replace the placeholders from the GitHub release.
class Ripsed < Formula
  desc "Fast, modern stream editor — like ripgrep is to grep, ripsed is to sed"
  homepage "https://github.com/dollspace-gay/ripsed"
  version "@VERSION@"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/dollspace-gay/ripsed/releases/download/v#{version}/ripsed-v#{version}-aarch64-apple-darwin.tar.gz"
      sha256 "@SHA256_MACOS_ARM@"
    else
      url "https://github.com/dollspace-gay/ripsed/releases/download/v#{version}/ripsed-v#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "@SHA256_MACOS_X64@"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/dollspace-gay/ripsed/releases/download/v#{version}/ripsed-v#{version}-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "@SHA256_LINUX_ARM@"
    else
      url "https://github.com/dollspace-gay/ripsed/releases/download/v#{version}/ripsed-v#{version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "@SHA256_LINUX_X64@"
    end
  end

  def install
    bin.install "ripsed"
    man1.install "ripsed.1"
    bash_completion.install "completions/ripsed.bash" => "ripsed"
    zsh_completion.install "completions/_ripsed"
    fish_completion.install "completions/ripsed.fish"
  end

  test do
    assert_match "thread", pipe_output("#{bin}/ripsed --pipe needle thread", "needle\n")
  end
end

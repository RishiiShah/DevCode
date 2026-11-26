# Packaging

The binary is `devcode`. The product name is not final, so every name below may change.

| Channel | Works today | What is needed to publish |
| :--- | :--- | :--- |
| `cargo install --git https://github.com/RishiiShah/DevCode` | yes | nothing |
| `install.sh` (release binaries) | after the first release publishes | `release.yml` stages binaries and checksums in a draft for a `v*` tag |
| Homebrew | `--HEAD` from a tap | create repo `homebrew-devcode`, copy `homebrew/devcode.rb` to `Formula/devcode.rb`; for stable installs add `url` and `sha256` |
| `pip` / `pipx` | build from source with maturin | upload wheels from the release to PyPI (needs a PyPI account or trusted publishing) |

## First beta release

1. Dispatch the release workflow from the candidate branch. Review the CI gate, native installer smoke tests, and the combined `release-assets` artifact.
2. Use a signed version tag matching `Cargo.toml`. The tag workflow stages a GitHub draft after all checks and builds pass.
3. Complete the signing, provenance, and hosting work in ARCH §6.3 before publishing the beta installer. Release checksums currently verify integrity and the installer still reads per-archive checksums.

The release matrix covers Intel and Apple Silicon macOS and glibc Linux on x86-64 and ARM64. Hosted rehearsal is pending.

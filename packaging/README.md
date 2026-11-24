# Packaging

The binary is `devcode`. The product name is not final, so every name below may change.

| Channel | Works today | What is needed to publish |
| :--- | :--- | :--- |
| `cargo install --git https://github.com/RishiiShah/DevCode` | yes | nothing |
| `install.sh` (release binaries) | after the first tag | push a `v*` tag; `release.yml` attaches binaries and checksums |
| Homebrew | `--HEAD` from a tap | create repo `homebrew-devcode`, copy `homebrew/devcode.rb` to `Formula/devcode.rb`; for stable installs add `url` and `sha256` |
| `pip` / `pipx` | build from source with maturin | upload wheels from the release to PyPI (needs a PyPI account or trusted publishing) |

Nothing here has been published. `release.yml` has not run yet.

# CodingPad Homebrew tap

The CodingPad repository is the tap. Homebrew loads `Casks/codingpad.rb` from
the default branch. Install and first-launch steps are in the
[CodingPad README](../README.md#macos).

## Update for a stable release

The release workflow generates `codingpad.rb` from the two macOS disk images
and uploads it alongside them. The cask pins the GitHub Release tag and a
SHA-256 checksum for each architecture.

1. From a fresh branch after the stable release is public, download the cask
   from that release. Substitute its tag for `v0.1.0`.

   ```sh
   gh release download v0.1.0 --repo joeizang/codingpad --pattern codingpad.rb --dir Casks --clobber
   ```

2. Open a pull request for `Casks/codingpad.rb`. Check that the version matches
   the release and both `.dmg` files are attached to it.
3. After merging, follow the README install steps on a clean Mac with Homebrew.
   Confirm CodingPad opens and can run a scratch script. Repeat on Apple silicon
   and Intel when both are available.

Only stable releases advance the tap. Revert the cask update to restore the
previous release.

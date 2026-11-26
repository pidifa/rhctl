## What's New

- Added `scripts/git/upgrade.sh`, a git upgrade script that replaces the old rhctl version with the new one (both `v1.0.10` and `1.0.10` forms) across user-specified files, then commits and pushes the changes to the main branch. Safe to re-run — it overwrites if it already exists.
- Fixed retrying the SSH channel on a transport disconnect during `transfer patch-env`, improving transfer reliability.

## Installation 🧰
### Homebrew (macOS / Linux)
Install:
```bash
brew install pidifa/rhctl/rhctl
```
Upgrade:
```bash
brew upgrade rhctl
```
### Linux (x86_64)
```bash
# Download
curl -LO https://github.com/pidifa/rhctl/releases/download/v1.0.10/rhctl-v1.0.10-linux-x86_64

# Make executable
chmod +x rhctl-v1.0.10-linux-x86_64

# Move to /usr/local/bin
sudo mv rhctl-v1.0.10-linux-x86_64 /usr/local/bin/rhctl
```
### macOS (x86_64)
```bash
# Download
curl -LO https://github.com/pidifa/rhctl/releases/download/v1.0.10/rhctl-v1.0.10-macos-x86_64

# Make executable
chmod +x rhctl-v1.0.10-macos-x86_64

# Move to /usr/local/bin
sudo mv rhctl-v1.0.10-macos-x86_64 /usr/local/bin/rhctl
```
### macOS (ARM64 / Apple Silicon)
```bash
# Download
curl -LO https://github.com/pidifa/rhctl/releases/download/v1.0.10/rhctl-v1.0.10-macos-arm64

# Make executable
chmod +x rhctl-v1.0.10-macos-arm64

# Move to /usr/local/bin
sudo mv rhctl-v1.0.10-macos-arm64 /usr/local/bin/rhctl
```

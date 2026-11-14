# What's New

- Fixed `=>` (contents-only) transfer merging into an existing remote directory instead of attempting to delete it, which previously failed with `Refusing to delete critical system path` for protected paths such as `/etc/systemd/system`.

### Installation 🧰
#### Homebrew (macOS / Linux)
```bash
brew install pidifa/rhctl/rhctl
```
#### Linux (x86_64)
```bash
# Download
curl -LO https://github.com/pidifa/rhctl/releases/download/v1.0.9/rhctl-v1.0.9-linux-x86_64

# Make executable
chmod +x rhctl-v1.0.9-linux-x86_64

# Move to /usr/local/bin
sudo mv rhctl-v1.0.9-linux-x86_64 /usr/local/bin/rhctl
```
#### macOS (x86_64)
```bash
# Download
curl -LO https://github.com/pidifa/rhctl/releases/download/v1.0.9/rhctl-v1.0.9-macos-x86_64

# Make executable
chmod +x rhctl-v1.0.9-macos-x86_64

# Move to /usr/local/bin
sudo mv rhctl-v1.0.9-macos-x86_64 /usr/local/bin/rhctl
```
#### macOS (ARM64 / Apple Silicon)
```bash
# Download
curl -LO https://github.com/pidifa/rhctl/releases/download/v1.0.9/rhctl-v1.0.9-macos-arm64

# Make executable
chmod +x rhctl-v1.0.9-macos-arm64

# Move to /usr/local/bin
sudo mv rhctl-v1.0.9-macos-arm64 /usr/local/bin/rhctl
```

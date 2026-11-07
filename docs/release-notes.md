=================================================== Release Branch
release/1.0.6

=================================================== Release Tag
Format:
    v[version]
Example:
    v1.0.6

=================================================== Release Title
Format:
    v[version]
Example:
    v1.0.6

=================================================== Release Description Format
Write the GitHub Release description manually (CI attaches artifacts and
updates the Homebrew tap; it does not generate or overwrite release notes).

Requires repository secret GIT_TOKEN with Contents write access to
saidake/homebrew-rhctl.

# What's New

## 🧪 Test Suite Fixes (`scripts/test.sh`)

- **Fixed `rhctl run` test**: Removed invalid `--host`, `--user`, `--ssh-port`,
  and `--password` arguments from the `rhctl run` test case. These options are
  not accepted by `rhctl run` — server connection details are sourced from
  `config.yml` via `target-servers`.
- **Fixed `transfer patch-env` test**: Replaced `assets/example1.txt` (which
  contained plain text `rhctl example file 1`, not valid `KEY=VALUE` format)
  with new `assets/example.env` containing proper `.env` entries:
  ```
  RHCTL_APP_NAME=test-app
  RHCTL_APP_PORT=8080
  RHCTL_DB_HOST=localhost
  RHCTL_DB_PORT=5432
  export RHCTL_DEBUG=true
  ```
- **Added `--silent` to all test commands**: Every `rhctl execute`, `rhctl
  transfer`, `rhctl patch`, and `rhctl run` invocation in the test suite now
  includes the `--silent` flag to suppress interactive prompts during automated
  runs.

## 📦 New Test Asset

- **`assets/example.env`**: Valid `.env` file for `patch-env` testing with
  `KEY=VALUE` entries, comment lines, and `export` prefix support.

### Installation 🧰
#### Homebrew (macOS / Linux)
```bash
brew install saidake/rhctl/rhctl
```
#### Linux (x86_64)
```bash
# Download
curl -LO https://github.com/saidake/rhctl/releases/download/v1.0.6/rhctl-v1.0.6-linux-x86_64

# Make executable
chmod +x rhctl-v1.0.6-linux-x86_64

# Move to /usr/local/bin
sudo mv rhctl-v1.0.6-linux-x86_64 /usr/local/bin/rhctl
```
#### macOS (x86_64)
```bash
# Download
curl -LO https://github.com/saidake/rhctl/releases/download/v1.0.6/rhctl-v1.0.6-macos-x86_64

# Make executable
chmod +x rhctl-v1.0.6-macos-x86_64

# Move to /usr/local/bin
sudo mv rhctl-v1.0.6-macos-x86_64 /usr/local/bin/rhctl
```
#### macOS (ARM64 / Apple Silicon)
```bash
# Download
curl -LO https://github.com/saidake/rhctl/releases/download/v1.0.6/rhctl-v1.0.6-macos-arm64

# Make executable
chmod +x rhctl-v1.0.6-macos-arm64

# Move to /usr/local/bin
sudo mv rhctl-v1.0.6-macos-arm64 /usr/local/bin/rhctl
```

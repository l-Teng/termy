#!/usr/bin/env bash
set -euo pipefail

check_forbidden_dep() {
  local crate="$1"
  local forbidden_dep="$2"

  if cargo tree -p "$crate" | rg -q "\b${forbidden_dep} v"; then
    echo "Boundary check failed: ${crate} must not depend on ${forbidden_dep}" >&2
    exit 1
  fi
}

check_forbidden_target_dep() {
  local crate="$1"
  local target="$2"
  local forbidden_dep="$3"

  if cargo tree -p "$crate" --target "$target" | rg -q "\b${forbidden_dep} v"; then
    echo "Boundary check failed: ${crate} must not depend on ${forbidden_dep} for ${target}" >&2
    exit 1
  fi
}

check_required_target_dep() {
  local crate="$1"
  local target="$2"
  local required_dep="$3"

  if ! cargo tree -p "$crate" --target "$target" | rg -q "\b${required_dep} v"; then
    echo "Boundary check failed: ${crate} must depend on ${required_dep} for ${target}" >&2
    exit 1
  fi
}

require_path() {
  local path="$1"

  if [[ ! -e "$path" ]]; then
    echo "Boundary check failed: required project path is missing: $path" >&2
    exit 1
  fi
}

forbid_pattern() {
  local pattern="$1"
  local path="$2"
  local message="$3"

  if rg -n "$pattern" "$path" >/dev/null; then
    echo "Boundary check failed: $message" >&2
    rg -n "$pattern" "$path" >&2
    exit 1
  fi
}

require_pattern() {
  local pattern="$1"
  local path="$2"
  local message="$3"

  if ! rg -n "$pattern" "$path" >/dev/null; then
    echo "Boundary check failed: $message" >&2
    exit 1
  fi
}

require_issue_url_for_pattern() {
  local pattern="$1"
  local path="$2"
  local message="$3"
  local matches
  local violations

  matches="$(rg -n "$pattern" "$path" || true)"
  if [[ -z "$matches" ]]; then
    return
  fi

  violations="$(printf '%s\n' "$matches" | rg -v 'https://github\.com/.*/issues/[0-9]+' || true)"
  if [[ -n "$violations" ]]; then
    echo "Boundary check failed: $message" >&2
    printf '%s\n' "$violations" >&2
    exit 1
  fi
}

require_ignored_test_budget() {
  local max_ignored_tests=10
  local ignored_count

  ignored_count="$(rg -n '#\[ignore' crates | wc -l | tr -d ' ')"
  if (( ignored_count > max_ignored_tests )); then
    echo "Boundary check failed: ignored test count is ${ignored_count}, max is ${max_ignored_tests}" >&2
    rg -n '#\[ignore' crates >&2
    exit 1
  fi
}

require_crate_readme_metadata() {
  local crate_dir="$1"
  local readme="$crate_dir/README.md"

  require_path "$readme"
  require_pattern '^## Owner$' \
    "$readme" \
    "$readme must document the crate owner boundary"
  require_pattern '^## Validation$' \
    "$readme" \
    "$readme must document validation commands"
  require_pattern '^cargo test -p ' \
    "$readme" \
    "$readme must document a cargo test command"
  require_pattern '^## Forbidden Dependencies$' \
    "$readme" \
    "$readme must document forbidden dependencies"
}

require_path "crates/desktop_app/Cargo.toml"
require_path "scripts/build-dmg.sh"
require_path "scripts/build-setup.ps1"
require_path "scripts/build-linux.sh"
require_path "scripts/check-platform-builds.sh"
require_path "crates/README.md"
require_path "scripts/README.md"
require_path "docs/architecture/project-layout.md"
require_path "docs/architecture/release-packaging.md"

while IFS= read -r manifest; do
  crate_dir="$(dirname "$manifest")"
  require_crate_readme_metadata "$crate_dir"
done < <(find crates -mindepth 2 -maxdepth 2 -name Cargo.toml | sort)

forbid_pattern 'macos/scripts|macos/dist' \
  ".github/workflows/release.yml" \
  "release workflow must use the current scripts/ packaging paths and Termy artifact names"

require_pattern './scripts/build-dmg\.sh' \
  ".github/workflows/release.yml" \
  "release workflow must call scripts/build-dmg.sh"
require_pattern 'dist/Termy-\$\{\{ env.VERSION \}\}-macos-\$\{\{ matrix.arch \}\}\.dmg' \
  ".github/workflows/release.yml" \
  "release workflow must upload the documented macOS DMG path"
forbid_pattern '^wry = ' \
  "crates/desktop_app/Cargo.toml" \
  "desktop app must not reintroduce the removed embedded browser runtime"
forbid_pattern '^gtk = ' \
  "crates/desktop_app/Cargo.toml" \
  "desktop app must not carry an unused Linux GTK dependency"
forbid_pattern 'WebView2|MicrosoftEdgeWebView2' \
  "scripts/build-setup.ps1" \
  "Windows setup must not bootstrap a browser runtime"
forbid_pattern 'WebView2|MicrosoftEdgeWebView2' \
  "scripts/installer/termy.iss" \
  "Windows installer must not package a browser runtime"
forbid_pattern 'webkit2gtk|GDK_BACKEND' \
  "scripts/build-linux.sh" \
  "Linux packages must not carry browser runtime requirements"
forbid_pattern 'webkit2gtk|GDK_BACKEND' \
  "scripts/install-linux.sh" \
  "Linux installer must not carry browser runtime requirements"
require_pattern 'pkg-config' \
  ".github/workflows/architecture-checks.yml" \
  "architecture checks must install pkg-config for Linux desktop builds"
require_pattern 'pkg-config' \
  ".github/workflows/release.yml" \
  "release workflow must install pkg-config for Linux desktop builds"
require_pattern './scripts/check-platform-builds\.sh --native' \
  ".github/workflows/architecture-checks.yml" \
  "architecture checks must run the shared native platform verifier"
require_pattern 'cargo check -p termy -p termy_cli' \
  "scripts/check-platform-builds.sh" \
  "platform verifier must check desktop and CLI crates"
require_pattern 'TERMY_CHECK_XWIN_MSVC' \
  "scripts/check-platform-builds.sh" \
  "platform verifier must expose an opt-in Windows MSVC cross-check"
require_pattern 'cargo xwin check --cross-compiler clang' \
  "scripts/check-platform-builds.sh" \
  "platform verifier must use the working cargo-xwin clang backend for MSVC checks"
require_pattern 'cp "\$BINARY_PATH" "\$STAGING_DIR/\$APP_NAME_LOWER/termy-bin"' \
  "scripts/build-linux.sh" \
  "Linux tarballs must ship the real GUI binary as termy-bin behind the launcher"
require_pattern 'rm -f "\$INSTALL_DIR/termy" "\$INSTALL_DIR/termy-bin" "\$INSTALL_DIR/termy-cli"' \
  "scripts/build-linux.sh" \
  "Linux tarball installer must unlink existing install targets before writing replacements"
require_pattern 'rm -f "\$INSTALL_DIR/termy" "\$INSTALL_DIR/termy-bin" "\$INSTALL_DIR/termy-cli"' \
  "scripts/install-linux.sh" \
  "Linux install helper must unlink existing install targets before writing replacements"
require_pattern 'cp "\$CLI_BINARY_PATH" "\$INSTALL_DIR/termy-cli"' \
  "scripts/install-linux.sh" \
  "Linux install helper must install the CLI sibling needed by desktop delegation"
require_pattern '\|\| true' \
  "scripts/install-linux.sh" \
  "Linux install helper release asset fallback must survive grep misses under pipefail"
require_pattern 'grep -Eo.*\|\| true' \
  "scripts/install-linux.sh" \
  "Linux install helper portable release JSON extraction must survive missing keys under pipefail"
require_pattern 'grep -Ev.*x86_64.*aarch64' \
  "scripts/install-linux.sh" \
  "Linux install helper generic fallback must not install an asset for the wrong architecture"
check_forbidden_dep "termy_command_core" "gpui"
check_forbidden_dep "termy_command_core" "termy_config_core"
check_forbidden_dep "termy_config_core" "termy_themes"
check_forbidden_dep "termy_cli_install_core" "gpui"
check_forbidden_dep "termy_cli" "gpui"
check_forbidden_dep "termy_core" "gpui"
check_forbidden_dep "termy_ffi" "gpui"
check_forbidden_dep "termy_ffi" "termy_terminal_ui"
check_forbidden_dep "termy_plugin_runtime" "gpui"
check_forbidden_dep "termy_plugin_runtime" "termy_command_core"
check_forbidden_dep "termy_plugin_runtime" "termy_config_core"
check_forbidden_dep "termy_plugin_runtime" "termy_terminal_ui"
check_forbidden_dep "termy_ssh_core" "gpui"
check_forbidden_dep "termy_ssh_core" "termy_terminal_ui"
check_forbidden_dep "termy_ui" "termy_terminal_ui"
check_forbidden_dep "termy_ui" "termy_config_core"
check_forbidden_dep "termy_ui" "termy_command_core"
check_forbidden_dep "termy_ui" "termy_plugin_runtime"
check_forbidden_dep "termy_ui" "termy_ssh_core"

require_issue_url_for_pattern \
  'clippy::cognitive_complexity' \
  "crates" \
  "clippy::cognitive_complexity allows must link a tracking issue on the same line"
require_ignored_test_budget

cargo run -p xtask -- generate-keybindings-doc --check
cargo run -p xtask -- generate-config-doc --check
cargo run -p xtask -- check-dependency-policy

"$(dirname "${BASH_SOURCE[0]}")/check-file-sizes.sh"

echo "Boundary checks passed"

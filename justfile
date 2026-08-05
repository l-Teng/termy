set shell := ["bash", "-cu"]

# Show available recipes
@default:
    just --list

run:
    cargo run -p termy --release

# Compare the terminal engines, enforce Tmon's snapshot baseline, and write a text report.
benchmark-tmon:
    #!/usr/bin/env bash
    set -euo pipefail
    report="${TMON_BENCH_OUTPUT:-tmon-alacritty-benchmark.txt}"
    {
      rustc --version
      echo
      cargo run --locked --quiet -p tmon --release --example engine_compare
    } 2>&1 | tee "$report"
    gate_status=0
    set +e
    {
      echo
      cargo run --locked --quiet --release --manifest-path tools/tmon-revision-gate/Cargo.toml
    } 2>&1 | tee -a "$report"
    gate_status=$?
    set -e
    {
      echo
      TMON_BENCH_ALLOCATIONS_ONLY=1 cargo run --locked --quiet -p tmon --release --example engine_compare --features benchmark-allocations
    } 2>&1 | tee -a "$report"
    exit "$gate_status"

run-cli *args:
    cargo run --bin termy-cli --release {{ args }}

# Build and run the experimental native macOS SwiftUI host (macos/)
run-macos *args:
    cargo macos {{ args }}

# Run native macOS Swift config/schema parity tests
test-macos-config:
    ./macos/scripts/check-config-matrix.sh

# Run native macOS stress-focused Swift tests
test-macos-stress *args:
    ./macos/scripts/stress-native.sh {{ args }}

# Check native macOS benchmark summary against regression gates
check-macos-performance *args:
    ./macos/scripts/check-performance-gates.sh {{ args }}

# Check native macOS staged app launch, RSS, and idle CPU gates
check-macos-launch *args:
    ./macos/scripts/check-launch-gate.sh {{ args }}

# Check native macOS release metadata, signing hooks, and optional staged app bundle
check-macos-release *args:
    ./macos/scripts/check-release-readiness.sh {{ args }}

test:
    cargo test -p termy --release

# All workspace crate tests (release). Target for CI — see docs/engineering/roadmap.md E0.1
test-workspace:
    cargo test --workspace --release

# Format check (no writes). Target for CI — see docs/engineering/roadmap.md E0.2
fmt-check:
    cargo fmt --all -- --check

# Local gate closest to target CI (see docs/engineering/roadmap.md E0.3)
validate: check fmt-check test-workspace check-boundaries
    cargo clippy --workspace --all-targets -- -D warnings

dev:
    cargo watch -x "run -p termy --release"

build:
    cargo build -p termy --release

check:
    cargo check --workspace

clean:
    cargo clean --workspace && rm -rf ./target

# Generate macOS .icns file from assets/termy_icon@1024px.png
generate-icon:
    ./scripts/generate-icon.sh

# Build the GPUI Termy app bundle and DMG (unsigned by default)
# Example:

# just build-dmg -- --version 0.1.0 --arch arm64 --sign-identity "Developer ID Application: ..."
build-dmg *args:
    set -- {{ args }}; \
    if [ "${1-}" = "--" ]; then shift; fi; \
    ./scripts/build-dmg.sh "$@"

# Build Windows Setup.exe via Inno Setup
# Example:

# just build-setup -- -Version 0.1.0 -Arch x64 -Target x86_64-pc-windows-msvc
build-setup *args:
    set -- {{ args }}; \
    if [ "${1-}" = "--" ]; then shift; fi; \
    if command -v powershell >/dev/null 2>&1; then \
      powershell -NoProfile -ExecutionPolicy Bypass -File ./scripts/build-setup.ps1 "$@"; \
    elif command -v powershell.exe >/dev/null 2>&1; then \
      powershell.exe -NoProfile -ExecutionPolicy Bypass -File ./scripts/build-setup.ps1 "$@"; \
    elif [ -x /c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe ]; then \
      /c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe -NoProfile -ExecutionPolicy Bypass -File ./scripts/build-setup.ps1 "$@"; \
    else \
      echo "PowerShell not found. Install PowerShell or run scripts/build-setup.ps1 directly from PowerShell."; \
      exit 127; \
    fi

# Render the local AUR PKGBUILD. Optional version defaults to crates/desktop_app/Cargo.toml.
aur-pkgbuild version="":
    ./scripts/aur/build-local.sh render "{{ version }}"

# Build the local AUR package under target/aur. Optional version defaults to crates/desktop_app/Cargo.toml.
aur-build version="":
    ./scripts/aur/build-local.sh build "{{ version }}"

# Build and install the local AUR package. Optional version defaults to crates/desktop_app/Cargo.toml.
aur-install version="":
    ./scripts/aur/build-local.sh install "{{ version }}"

generate-keybindings-doc:
    cargo run -p xtask -- generate-keybindings-doc

generate-config-doc:
    cargo run -p xtask -- generate-config-doc

check-keybindings-doc:
    cargo run -p xtask -- generate-keybindings-doc --check

check-config-doc:
    cargo run -p xtask -- generate-config-doc --check

check-boundaries:
    ./scripts/check-boundaries.sh

check-file-sizes:
    ./scripts/check-file-sizes.sh

test-tmux-integration:
    cargo test -p termy_terminal_ui --test tmux_split_integration -- --ignored --nocapture --test-threads=1

# Bump version in desktop app + cli Cargo.toml. Kind: major | minor | patch
bump kind:
    #!/usr/bin/env bash
    set -euo pipefail
    KIND="{{ kind }}"
    CURRENT=$(grep -m1 '^version = ' crates/desktop_app/Cargo.toml | sed -E 's/version = "(.*)"/\1/')
    IFS=. read -r MAJOR MINOR PATCH <<<"$CURRENT"
    case "$KIND" in
      major) MAJOR=$((MAJOR + 1)); MINOR=0; PATCH=0 ;;
      minor) MINOR=$((MINOR + 1)); PATCH=0 ;;
      patch) PATCH=$((PATCH + 1)) ;;
      *) echo "usage: just bump <major|minor|patch>"; exit 1 ;;
    esac
    NEW="$MAJOR.$MINOR.$PATCH"
    export NEW
    perl -i -pe '$done ||= s/^version = ".*"/version = "$ENV{NEW}"/' crates/desktop_app/Cargo.toml
    perl -i -pe '$done ||= s/^version = ".*"/version = "$ENV{NEW}"/' crates/cli/Cargo.toml
    echo "Bumped $CURRENT -> $NEW"

# Set exact version in desktop app + cli Cargo.toml (e.g. just set-version 0.2.12)
set-version version:
    #!/usr/bin/env bash
    set -euo pipefail
    NEW="{{ version }}"
    if ! [[ "$NEW" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
      echo "usage: just set-version <major.minor.patch> (got: $NEW)"; exit 1
    fi
    CURRENT=$(grep -m1 '^version = ' crates/desktop_app/Cargo.toml | sed -E 's/version = "(.*)"/\1/')
    export NEW
    perl -i -pe '$done ||= s/^version = ".*"/version = "$ENV{NEW}"/' crates/desktop_app/Cargo.toml
    perl -i -pe '$done ||= s/^version = ".*"/version = "$ENV{NEW}"/' crates/cli/Cargo.toml
    echo "Set $CURRENT -> $NEW"

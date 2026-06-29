#!/usr/bin/env bash

set -euo pipefail

usage() {
  cat >&2 <<'EOF'
Usage: build_macos_dmg.sh --archive PATH --target TARGET --version VERSION --output PATH

Builds a macOS DMG containing:
  - install.command
  - install.sh
  - pfterminal-package-<target>.tar.gz
  - pfterminal-package_SHA256SUMS

The DMG installer uses the bundled package archive and does not need to fetch
release assets from GitHub.
EOF
}

archive_path=""
target=""
version=""
output_path=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --archive)
      archive_path="${2:-}"
      shift 2
      ;;
    --target)
      target="${2:-}"
      shift 2
      ;;
    --version)
      version="${2:-}"
      shift 2
      ;;
    --output)
      output_path="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage
      exit 2
      ;;
  esac
done

if [[ -z "$archive_path" || -z "$target" || -z "$version" || -z "$output_path" ]]; then
  usage
  exit 2
fi

if [[ ! -f "$archive_path" ]]; then
  echo "Package archive does not exist: $archive_path" >&2
  exit 1
fi

if [[ "$target" != *apple-darwin ]]; then
  echo "DMG target must be a macOS target, got: $target" >&2
  exit 2
fi

if ! command -v hdiutil >/dev/null 2>&1; then
  echo "hdiutil is required to build a macOS DMG." >&2
  exit 1
fi

if ! command -v shasum >/dev/null 2>&1; then
  echo "shasum is required to build a macOS DMG." >&2
  exit 1
fi

case "$target" in
  aarch64-apple-darwin)
    platform_label="macOS Apple Silicon"
    ;;
  x86_64-apple-darwin)
    platform_label="macOS Intel"
    ;;
  *)
    platform_label="macOS"
    ;;
esac

repo_root="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
archive_name="$(basename "$archive_path")"
expected_archive_name="pfterminal-package-${target}.tar.gz"
if [[ "$archive_name" != "$expected_archive_name" ]]; then
  echo "Archive name must be $expected_archive_name, got: $archive_name" >&2
  exit 2
fi

mkdir -p "$(dirname "$output_path")"
rm -f "$output_path"

work_dir="$(mktemp -d)"
trap 'rm -rf "$work_dir"' EXIT

staging_dir="$work_dir/PFTerminal Installer"
mkdir -p "$staging_dir"

cp "$archive_path" "$staging_dir/$archive_name"
cp "$repo_root/scripts/install/install.sh" "$staging_dir/install.sh"
chmod 0755 "$staging_dir/install.sh"

archive_sha256="$(shasum -a 256 "$archive_path" | awk '{ print $1 }')"
printf '%s  %s\n' "$archive_sha256" "$archive_name" > "$staging_dir/pfterminal-package_SHA256SUMS"

cat > "$staging_dir/install.command" <<EOF
#!/bin/sh
set -eu

SCRIPT_DIR=\$(CDPATH= cd -- "\$(dirname -- "\$0")" && pwd)

export PFTERMINAL_RELEASE="${version}"
export PFTERMINAL_PACKAGE_ARCHIVE="\$SCRIPT_DIR/${archive_name}"
export PFTERMINAL_CHECKSUM_MANIFEST="\$SCRIPT_DIR/pfterminal-package_SHA256SUMS"

exec /bin/sh "\$SCRIPT_DIR/install.sh" "\$@"
EOF
chmod 0755 "$staging_dir/install.command"

cat > "$staging_dir/README.txt" <<EOF
PFTerminal ${version} for ${platform_label}

Double-click install.command to install the pfterminal command.

Default install locations:
  Command launcher: \$HOME/.local/bin/pfterminal
  PFTerminal state: \$HOME/.pfterminal

The installer leaves any existing stock codex command alone. It installs the
bundled package archive from this DMG and verifies it against
pfterminal-package_SHA256SUMS before installation.

Advanced terminal install:
  sh /Volumes/PFTerminal-${version}-${target}/install.command
EOF

volume_name="PFTerminal-${version}-${target}"
hdiutil create \
  -volname "$volume_name" \
  -srcfolder "$staging_dir" \
  -ov \
  -format UDZO \
  "$output_path"

echo "Built $output_path"

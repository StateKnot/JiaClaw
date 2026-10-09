#!/bin/sh
# Install a versioned official release without sudo or modifying configuration.
set -eu
fail() { printf '%s\n' "JiaClaw: $*" >&2; exit 1; }
if [ "${1:-}" = '--help' ]; then
  printf '%s\n' 'Usage: install.sh vX.Y.Z' 'JIACLAW_INSTALL_DIR defaults to ~/.local/bin.'
  exit 0
fi
[ "$#" -eq 1 ] || fail 'provide an explicit release version, e.g. v0.1.0'
version=$1
printf '%s\n' "$version" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$' || fail 'invalid version'
case "$(uname -s):$(uname -m)" in
  Linux:x86_64) target=x86_64-unknown-linux-gnu ;;
  Linux:aarch64|Linux:arm64) target=aarch64-unknown-linux-gnu ;;
  Darwin:x86_64) target=x86_64-apple-darwin ;;
  Darwin:arm64) target=aarch64-apple-darwin ;;
  *) fail 'supported platforms: Linux/macOS, x86_64/arm64' ;;
esac
command -v curl >/dev/null || fail 'curl is required'
command -v tar >/dev/null || fail 'tar is required'
if command -v sha256sum >/dev/null; then hash=sha256sum
elif command -v shasum >/dev/null; then hash=shasum
else fail 'sha256sum or shasum is required'; fi
install_dir=${JIACLAW_INSTALL_DIR:-"$HOME/.local/bin"}
mkdir -p "$install_dir"
[ ! -d "$install_dir/jiaclaw" ] || fail 'destination is a directory'
tmp=$(mktemp -d)
staged=''
cleanup() { rm -rf "$tmp"; [ -z "$staged" ] || rm -f "$staged"; }
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
asset="jiaclaw-${version}-${target}.tar.gz"
base="https://github.com/StateKnot/JiaClaw/releases/download/${version}"
for file in "$asset" SHA256SUMS; do
  curl --fail --silent --show-error --location --proto '=https' --proto-redir '=https' --tlsv1.2 \
    --connect-timeout 10 --max-time 120 "$base/$file" --output "$tmp/$file"
done
expected=$(awk -v file="$asset" '$2 == file {print $1}' "$tmp/SHA256SUMS")
printf '%s\n' "$expected" | grep -Eq '^[0-9a-f]{64}$' || fail 'release checksum missing or malformed'
if [ "$hash" = shasum ]; then actual=$(shasum -a 256 "$tmp/$asset" | awk '{print $1}')
else actual=$(sha256sum "$tmp/$asset" | awk '{print $1}'); fi
[ "$actual" = "$expected" ] || fail 'checksum mismatch; existing installation retained'
tar -xzf "$tmp/$asset" -C "$tmp" jiaclaw
[ -f "$tmp/jiaclaw" ] && [ ! -L "$tmp/jiaclaw" ] || fail 'release binary is not a regular file'
reported_version=$("$tmp/jiaclaw" version) || fail 'binary cannot run on this platform; existing installation retained'
[ "$(printf '%s\n' "$reported_version" | sed -n '1p')" = "JiaClaw $version" ] || \
  fail 'binary version does not match requested release; existing installation retained'
staged=$(mktemp "$install_dir/.jiaclaw-install.XXXXXX")
cat "$tmp/jiaclaw" > "$staged"
chmod 755 "$staged"
mv -f "$staged" "$install_dir/jiaclaw"
staged=''
printf '%s\n' "Installed $version to $install_dir/jiaclaw" 'Add this directory to PATH if needed. Configuration and workspace were preserved.'

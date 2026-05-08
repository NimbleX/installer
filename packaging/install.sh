#!/bin/sh
# Build and stage the Nimblex installer for inclusion in 03-Installer64.lzm
# (or wherever the Nimblex live-build pipeline expects pre-staged trees).
#
# Output layout (under $STAGE):
#   usr/bin/nimblex-installer
#   usr/libexec/nimblex-installer-helper
#   usr/libexec/nimblex-installer-helper-internal -> nimblex-installer-helper
#   usr/share/applications/nimblex-installer.desktop
#   usr/share/polkit-1/actions/org.nimblex.installer.policy
#
# Usage: STAGE=/path/to/stage packaging/install.sh

set -eu

STAGE="${STAGE:-./stage}"
PROFILE="${PROFILE:-release}"
HERE="$(cd "$(dirname "$0")/.." && pwd)"

case "$PROFILE" in
    release) cargo build --release ;;
    debug)   cargo build ;;
    *) echo "PROFILE must be release or debug" >&2; exit 2 ;;
esac

mkdir -p \
    "$STAGE/usr/bin" \
    "$STAGE/usr/libexec" \
    "$STAGE/usr/share/applications" \
    "$STAGE/usr/share/polkit-1/actions"

install -m 0755 "$HERE/target/$PROFILE/nimblex-installer"        "$STAGE/usr/bin/nimblex-installer"
install -m 0755 "$HERE/target/$PROFILE/nimblex-installer-helper" "$STAGE/usr/libexec/nimblex-installer-helper"
ln -sf nimblex-installer-helper "$STAGE/usr/libexec/nimblex-installer-helper-internal"
install -m 0644 "$HERE/packaging/share/applications/nimblex-installer.desktop" \
    "$STAGE/usr/share/applications/nimblex-installer.desktop"
install -m 0644 "$HERE/packaging/polkit/org.nimblex.installer.policy" \
    "$STAGE/usr/share/polkit-1/actions/org.nimblex.installer.policy"

echo "staged Nimblex installer in $STAGE" >&2

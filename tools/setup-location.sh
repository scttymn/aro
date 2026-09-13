#!/usr/bin/env bash
# Optional host location support. Installing packages does not enable location.
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: aro location [status|install]

  status   Check GeoClue, the desktop Location portal and host enablement.
  install  Install optional Arch location packages, then show their status.

Location uses the desktop portal and GeoClue. Installing these packages does
not enable location or grant Android apps access. No IP/weather fallback is used.
Android requests can also offer consent-based setup automatically on first use.
EOF
}

if (( $# > 1 )); then
  usage >&2
  exit 2
fi

case "${1:-status}" in
  -h|--help|help) usage; exit 0 ;;
  status) ;;
  install)
    if command -v omarchy >/dev/null; then
      omarchy pkg add geoclue xdg-desktop-portal
    elif command -v pacman >/dev/null; then
      sudo pacman -S --needed geoclue xdg-desktop-portal
    else
      echo 'Install GeoClue and xdg-desktop-portal with your distribution package manager.' >&2
      exit 1
    fi
    ;;
  *) usage >&2; exit 2 ;;
esac

if command -v pacman >/dev/null && pacman -Q geoclue >/dev/null 2>&1; then
  pacman -Q geoclue
else
  echo 'GeoClue: not installed (or not managed by pacman)'
fi
if timeout 5s busctl --user introspect org.freedesktop.portal.Desktop \
    /org/freedesktop/portal/desktop org.freedesktop.portal.Location >/dev/null 2>&1; then
  echo 'Desktop Location portal: available'
else
  echo 'Desktop Location portal: unavailable'
fi
enabled=$(timeout 2s gsettings get org.gnome.system.location enabled 2>/dev/null || true)
case "$enabled" in
  true) echo 'Host location preference: enabled' ;;
  false) echo 'Host location preference: disabled' ;;
  *) echo 'Host location preference: unavailable' ;;
esac
echo 'A live Android request is still required to verify consent and a location fix.'

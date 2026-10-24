#!/usr/bin/env bash
# Installs or updates sweepr on Arch Linux. It builds the package from source
# with makepkg, installs it with pacman (so `pacman -Rns sweepr` removes it
# cleanly), and turns on the timers.
#
#   curl -fsSL https://raw.githubusercontent.com/CratusGod/sweepr/main/install.sh | bash
#
# SWEEPR_REF picks a branch, tag or commit (default: main).
# SWEEPR_SRC overrides the git source; CI points it at the checked-out tree.
set -euo pipefail

work=""
cleanup() { if [[ -n $work ]]; then rm -rf "$work"; fi; }
trap cleanup EXIT

say() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m==>\033[0m %s\n' "$*" >&2; }
die() {
  printf '\033[1;31merror:\033[0m %s\n' "$*" >&2
  exit 1
}

# `curl | bash` leaves stdin on the pipe, so prompts read the terminal instead.
# With no terminal at all (CI), answer the defaults.
interactive() {
  if [[ -t 0 ]]; then
    "$@"
  elif { : </dev/tty; } 2>/dev/null; then
    "$@" </dev/tty
  else
    "$@" --noconfirm
  fi
}

main() {
  local src=${SWEEPR_SRC:-https://github.com/CratusGod/sweepr}
  local ref=${SWEEPR_REF:-main}

  [[ $EUID -ne 0 ]] || die "run this as your normal user, not root; it asks for sudo when it needs it"
  command -v pacman >/dev/null || die "sweepr is for Arch Linux, and pacman was not found"

  local missing
  mapfile -t missing < <(pacman -T git base-devel || true)
  if ((${#missing[@]})); then
    say "Installing build tools: ${missing[*]}"
    interactive sudo pacman -S --needed "${missing[@]}"
  fi

  work=$(mktemp -d)
  say "Downloading sweepr ($ref)"
  git clone --quiet "$src" "$work/sweepr"
  git -C "$work/sweepr" checkout --quiet "$ref"

  say "Building and installing (about a minute)"
  (cd "$work/sweepr/packaging/local" && interactive makepkg --syncdeps --install --force --nocheck)

  if systemctl --user daemon-reload 2>/dev/null; then
    systemctl --user enable --now sweepr.timer sweepr-tick.timer
  else
    warn "No user systemd session here. Later, run: systemctl --user enable --now sweepr.timer sweepr-tick.timer"
  fi

  say "$(sweepr --version) is installed. Scheduled runs only preview until you enable them:"
  echo "      sweepr scan -v    # see exactly what would move"
  echo "      sweepr enable     # let the monthly run move files"
}

main "$@"

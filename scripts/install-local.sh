#!/usr/bin/env bash
# Put the current build on PATH as `omachess`.
#
# A copy would go stale the moment the next build finished, which is exactly
# what happened once: ~/.local/bin/omachess sat a day behind the repository
# while the desktop entry (Exec=omachess) resolved to it, so launching from the
# app menu ran an old build and nothing behaved as described. A link cannot
# drift.
set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
binary="$repo/target/release/omachess"
target="${1:-$HOME/.local/bin/omachess}"

[ -x "$binary" ] || { echo "build it first: cargo build --release" >&2; exit 1; }

mkdir -p "$(dirname "$target")"
ln -sfn "$binary" "$target"
echo "linked $target -> $binary"
"$target" --version

#!/usr/bin/env bash
# Run omachess against an isolated dev state directory.
#
# All XDG paths are redirected into ./.devhome so a development build can never
# read or write the real review database. Reset the dev state with:
#     rm -rf .devhome
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
devhome="$root/.devhome"

mkdir -p "$devhome"/{data,config,cache,state}

# GTK reads its stylesheet from XDG_CONFIG_HOME, and Omarchy generates that
# sheet from the active theme. Link the real ones in so a development run still
# looks like the installed application; only application state is isolated.
for toolkit in gtk-4.0 gtk-3.0; do
    if [[ -d "$HOME/.config/$toolkit" && ! -e "$devhome/config/$toolkit" ]]; then
        ln -s "$HOME/.config/$toolkit" "$devhome/config/$toolkit"
    fi
done

# Piece sets are installed assets, not review state, so the real ones are
# shared into the dev profile rather than duplicated.
if [[ -d "$HOME/.local/share/omachess/pieces" ]]; then
    mkdir -p "$devhome/data/omachess"
    if [[ ! -e "$devhome/data/omachess/pieces" ]]; then
        ln -s "$HOME/.local/share/omachess/pieces" "$devhome/data/omachess/pieces"
    fi
fi

export XDG_DATA_HOME="$devhome/data"
export XDG_CONFIG_HOME="$devhome/config"
export XDG_CACHE_HOME="$devhome/cache"
export XDG_STATE_HOME="$devhome/state"
export OMACHESS_PROFILE=dev

exec cargo run --manifest-path "$root/Cargo.toml" --bin omachess -- "$@"

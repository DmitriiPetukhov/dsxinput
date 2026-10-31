#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
build_dir="$script_dir/target/release"
if [ -n "${PREFIX:-}" ]; then
    install_prefix=$PREFIX
else
    : "${HOME:?HOME must be set when PREFIX is not set}"
    install_prefix=$HOME/.local
fi

(
    cd -- "$script_dir"
    cargo build --release --locked --target-dir "$script_dir/target"
)

install -D -m 0755 -- "$build_dir/dsxinput" "$install_prefix/bin/dsxinput"

printf 'Installed dsxinput to %s\n' "$install_prefix"

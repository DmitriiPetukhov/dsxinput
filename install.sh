#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
build_dir=${DSXINPUT_BUILD_DIR:-"$script_dir/target/release"}
if [ -n "${PREFIX:-}" ]; then
    install_prefix=$PREFIX
else
    : "${HOME:?HOME must be set when PREFIX is not set}"
    install_prefix=$HOME/.local
fi

artifacts='dsxinput libdsxinput_overlay.so VK_LAYER_DSXINPUT_overlay.json'
for artifact in $artifacts; do
    if [ ! -f "$build_dir/$artifact" ]; then
        printf 'dsxinput installer: missing %s; build with cargo build --workspace --release --locked first\n' \
            "$build_dir/$artifact" >&2
        exit 1
    fi
done

install -D -m 0755 -- "$build_dir/dsxinput" "$install_prefix/bin/dsxinput"
install -D -m 0755 -- "$build_dir/libdsxinput_overlay.so" \
    "$install_prefix/lib/dsxinput/libdsxinput_overlay.so"
install -D -m 0644 -- "$build_dir/VK_LAYER_DSXINPUT_overlay.json" \
    "$install_prefix/lib/dsxinput/VK_LAYER_DSXINPUT_overlay.json"

printf 'Installed dsxinput to %s\n' "$install_prefix"

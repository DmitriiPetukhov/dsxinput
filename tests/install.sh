#!/bin/sh
set -eu

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
temp_root=$(mktemp -d)
trap 'rm -rf "$temp_root"' EXIT HUP INT TERM

fixture_dir="$temp_root/build output"
mkdir -p "$fixture_dir"
printf '%s\n' 'fixture wrapper' >"$fixture_dir/dsxinput"
printf '%s\n' 'fixture layer' >"$fixture_dir/libdsxinput_overlay.so"
printf '%s\n' '{"fixture":true}' >"$fixture_dir/VK_LAYER_DSXINPUT_overlay.json"

assert_install() {
    prefix=$1
    [ -f "$prefix/bin/dsxinput" ]
    [ -f "$prefix/lib/dsxinput/libdsxinput_overlay.so" ]
    [ -f "$prefix/lib/dsxinput/VK_LAYER_DSXINPUT_overlay.json" ]
    cmp "$fixture_dir/dsxinput" "$prefix/bin/dsxinput"
    cmp "$fixture_dir/libdsxinput_overlay.so" "$prefix/lib/dsxinput/libdsxinput_overlay.so"
    cmp "$fixture_dir/VK_LAYER_DSXINPUT_overlay.json" \
        "$prefix/lib/dsxinput/VK_LAYER_DSXINPUT_overlay.json"
    [ "$(stat -c '%a' "$prefix/bin/dsxinput")" = 755 ]
    [ "$(stat -c '%a' "$prefix/lib/dsxinput/libdsxinput_overlay.so")" = 755 ]
    [ "$(stat -c '%a' "$prefix/lib/dsxinput/VK_LAYER_DSXINPUT_overlay.json")" = 644 ]
}

default_prefix="$temp_root/home/.local"
mkdir -p "$temp_root/home"
HOME="$temp_root/home" DSXINPUT_BUILD_DIR="$fixture_dir" "$repo_root/install.sh"
assert_install "$default_prefix"

custom_prefix="$temp_root/custom prefix"
HOME="$temp_root/home" PREFIX="$custom_prefix" DSXINPUT_BUILD_DIR="$fixture_dir" \
    "$repo_root/install.sh"
assert_install "$custom_prefix"

printf '%s\n' 'installer layout and mode checks passed'

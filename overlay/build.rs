use std::{env, fs, path::PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let manifest = manifest_dir.join("vulkan-layer.json");
    println!("cargo:rerun-if-changed={}", manifest.display());

    // OUT_DIR is <profile>/build/<package-hash>/out for both native and
    // target-specific builds, so the manifest can be placed beside the cdylib.
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let profile_dir = out_dir
        .ancestors()
        .nth(3)
        .expect("Cargo OUT_DIR is nested below its profile directory");
    fs::copy(
        &manifest,
        profile_dir.join("VK_LAYER_DSXINPUT_overlay.json"),
    )
    .expect("copy Vulkan layer manifest beside cdylib");
}

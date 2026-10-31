use std::{env, fs, path::PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let manifest = manifest_dir.join("vulkan-layer.json");
    println!("cargo:rerun-if-changed={}", manifest.display());

    let shader_path = manifest_dir.join("shaders/compositor.wgsl");
    println!("cargo:rerun-if-changed={}", shader_path.display());

    let shader_source = fs::read_to_string(&shader_path)
        .unwrap_or_else(|error| panic!("read {}: {error}", shader_path.display()));
    let shader_module = naga::front::wgsl::parse_str(&shader_source).unwrap_or_else(|error| {
        panic!(
            "parse {}:\n{}",
            shader_path.display(),
            error.emit_to_string(&shader_source)
        )
    });
    let shader_info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&shader_module)
    .unwrap_or_else(|error| panic!("validate {}: {error}", shader_path.display()));

    // OUT_DIR is <profile>/build/<package-hash>/out for both native and
    // target-specific builds, so the manifest can be placed beside the cdylib.
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let profile_dir = out_dir
        .ancestors()
        .nth(3)
        .expect("Cargo OUT_DIR is nested below its profile directory");

    let mut spv_options = naga::back::spv::Options::default();
    spv_options.lang_version = (1, 0);
    for (shader_stage, entry_point, output_name) in [
        (
            naga::ShaderStage::Vertex,
            "compositor_vertex",
            "compositor.vert.spv",
        ),
        (
            naga::ShaderStage::Fragment,
            "compositor_fragment",
            "compositor.frag.spv",
        ),
    ] {
        let pipeline_options = naga::back::spv::PipelineOptions {
            shader_stage,
            entry_point: entry_point.to_owned(),
        };
        let words = naga::back::spv::write_vec(
            &shader_module,
            &shader_info,
            &spv_options,
            Some(&pipeline_options),
        )
        .unwrap_or_else(|error| {
            panic!(
                "compile {} entry point {entry_point}: {error}",
                shader_path.display()
            )
        });
        let mut bytes = Vec::with_capacity(words.len() * 4);
        for word in words {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        fs::write(out_dir.join(output_name), bytes)
            .unwrap_or_else(|error| panic!("write {output_name}: {error}"));
    }

    fs::copy(
        &manifest,
        profile_dir.join("VK_LAYER_DSXINPUT_overlay.json"),
    )
    .expect("copy Vulkan layer manifest beside cdylib");
}

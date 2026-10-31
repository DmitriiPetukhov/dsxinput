//! Discover and configure the Vulkan layer for the launched game.
use std::{
    env,
    ffi::{OsStr, OsString},
    io,
    os::unix::ffi::OsStrExt,
    path::Path,
};

const LAYER_NAME: &str = "VK_LAYER_DSXINPUT_overlay";
const MANIFEST_FILE: &str = "VK_LAYER_DSXINPUT_overlay.json";
const LIBRARY_FILE: &str = "libdsxinput_overlay.so";

pub(crate) struct LayerArtifacts {
    pub(crate) directory: std::path::PathBuf,
    pub(crate) name: OsString,
}

impl LayerArtifacts {
    pub(crate) fn discover_from(executable: &Path) -> io::Result<Option<Self>> {
        let Some(executable_dir) = executable.parent() else {
            return Ok(None);
        };
        let mut candidates = vec![executable_dir.to_path_buf()];
        if let Some(prefix) = executable_dir.parent() {
            candidates.push(prefix.join("lib/dsxinput"));
        }

        for directory in candidates {
            if directory.join(MANIFEST_FILE).is_file() && directory.join(LIBRARY_FILE).is_file() {
                return Ok(Some(Self {
                    directory,
                    name: OsString::from(LAYER_NAME),
                }));
            }
        }
        Ok(None)
    }

    pub(crate) fn environment_updates(
        &self,
        existing: impl Fn(&str) -> Option<OsString>,
    ) -> io::Result<Vec<(OsString, OsString)>> {
        let mut updates = vec![(
            OsString::from("VK_ADD_LAYER_PATH"),
            append_path(existing("VK_ADD_LAYER_PATH").as_deref(), &self.directory)?,
        )];
        if let Some(value) = existing("VK_LAYER_PATH") {
            updates.push((
                OsString::from("VK_LAYER_PATH"),
                append_path(Some(&value), &self.directory)?,
            ));
        }
        updates.push((
            OsString::from("VK_INSTANCE_LAYERS"),
            append_layer(existing("VK_INSTANCE_LAYERS").as_deref(), &self.name),
        ));
        Ok(updates)
    }
}

fn append_path(existing: Option<&OsStr>, directory: &Path) -> io::Result<OsString> {
    let mut paths = existing
        .map(env::split_paths)
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    if !paths.iter().any(|path| path == directory) {
        paths.push(directory.to_path_buf());
    }
    env::join_paths(paths).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("Vulkan layer directory cannot be represented in a path list: {error}"),
        )
    })
}

fn append_layer(existing: Option<&OsStr>, layer: &OsStr) -> OsString {
    let present = existing.is_some_and(|value| {
        value
            .as_bytes()
            .split(|byte| *byte == b':')
            .any(|entry| entry == layer.as_bytes())
    });
    let mut result = existing.map(OsStr::to_os_string).unwrap_or_default();
    if !present {
        if !result.is_empty() {
            result.push(":");
        }
        result.push(layer);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        env, fs,
        os::unix::ffi::OsStringExt,
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
    };

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let path = env::temp_dir().join(format!(
                "dsxinput-layer-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn add_layer(directory: &Path) {
            fs::create_dir_all(directory).unwrap();
            fs::write(directory.join("VK_LAYER_DSXINPUT_overlay.json"), b"{}").unwrap();
            fs::write(directory.join("libdsxinput_overlay.so"), b"layer").unwrap();
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn discovers_layer_beside_executable_before_install_fallback() {
        let temp = TestDir::new();
        let executable_dir = temp.0.join("bin");
        let installed_dir = temp.0.join("lib/dsxinput");
        TestDir::add_layer(&executable_dir);
        TestDir::add_layer(&installed_dir);
        let executable = executable_dir.join("dsxinput");

        let layer = LayerArtifacts::discover_from(&executable).unwrap().unwrap();

        assert_eq!(layer.directory, executable_dir);
    }

    #[test]
    fn discovers_layer_in_prefix_lib_when_not_beside_executable() {
        let temp = TestDir::new();
        let executable = temp.0.join("prefix/bin/dsxinput");
        let installed_dir = temp.0.join("prefix/lib/dsxinput");
        TestDir::add_layer(&installed_dir);

        let layer = LayerArtifacts::discover_from(&executable).unwrap().unwrap();

        assert_eq!(layer.directory, installed_dir);
    }

    #[test]
    fn rejects_incomplete_layer_artifacts() {
        let temp = TestDir::new();
        let executable_dir = temp.0.join("bin");
        fs::create_dir_all(&executable_dir).unwrap();
        fs::write(executable_dir.join("VK_LAYER_DSXINPUT_overlay.json"), b"{}").unwrap();

        assert!(
            LayerArtifacts::discover_from(&executable_dir.join("dsxinput"))
                .unwrap()
                .is_none()
        );

        let fallback = temp.0.join("prefix/lib/dsxinput");
        fs::create_dir_all(&fallback).unwrap();
        fs::write(fallback.join("libdsxinput_overlay.so"), b"layer").unwrap();
        assert!(
            LayerArtifacts::discover_from(&temp.0.join("prefix/bin/dsxinput"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn discovery_and_environment_support_spaces_and_non_utf8_paths() {
        let temp = TestDir::new();
        let non_utf8 = OsString::from_vec(b"bin-\xff with spaces".to_vec());
        let executable_dir = temp.0.join(non_utf8);
        TestDir::add_layer(&executable_dir);
        let executable = executable_dir.join("dsxinput");
        let layer = LayerArtifacts::discover_from(&executable).unwrap().unwrap();
        let updates = layer.environment_updates(|_| None).unwrap();
        let add_path = updates
            .iter()
            .find(|(key, _)| key == "VK_ADD_LAYER_PATH")
            .unwrap();

        assert_eq!(
            env::split_paths(&add_path.1).next().unwrap(),
            executable_dir
        );
        assert!(add_path.1.as_os_str().as_bytes().contains(&0xff));
    }

    #[test]
    fn environment_updates_append_only_missing_values_and_preserve_existing() {
        let layer_dir = PathBuf::from("/overlay with spaces");
        let layer = LayerArtifacts {
            directory: layer_dir.clone(),
            name: OsString::from("VK_LAYER_DSXINPUT_overlay"),
        };
        let updates = layer
            .environment_updates(|key| match key {
                "VK_ADD_LAYER_PATH" => Some(OsString::from("/vendor")),
                "VK_LAYER_PATH" => Some(OsString::from("/loader")),
                "VK_INSTANCE_LAYERS" => {
                    Some(OsString::from("VK_LAYER_OTHER:VK_LAYER_DSXINPUT_overlay"))
                }
                _ => None,
            })
            .unwrap();
        let value = |name: &str| {
            updates
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value)
                .unwrap()
        };

        assert_eq!(
            env::split_paths(value("VK_ADD_LAYER_PATH")).collect::<Vec<_>>(),
            [PathBuf::from("/vendor"), layer_dir.clone()]
        );
        assert_eq!(
            env::split_paths(value("VK_LAYER_PATH")).collect::<Vec<_>>(),
            [PathBuf::from("/loader"), layer_dir]
        );
        assert_eq!(
            value("VK_INSTANCE_LAYERS"),
            "VK_LAYER_OTHER:VK_LAYER_DSXINPUT_overlay"
        );
    }

    #[test]
    fn environment_updates_do_not_duplicate_existing_layer_paths_or_names() {
        let layer_dir = PathBuf::from("/overlay");
        let layer = LayerArtifacts {
            directory: layer_dir.clone(),
            name: OsString::from("VK_LAYER_DSXINPUT_overlay"),
        };
        let existing_paths = env::join_paths([PathBuf::from("/vendor"), layer_dir]).unwrap();
        let updates = layer
            .environment_updates(|key| match key {
                "VK_ADD_LAYER_PATH" | "VK_LAYER_PATH" => Some(existing_paths.clone()),
                "VK_INSTANCE_LAYERS" => {
                    Some(OsString::from("VK_LAYER_OTHER:VK_LAYER_DSXINPUT_overlay"))
                }
                _ => None,
            })
            .unwrap();

        assert_eq!(updates[0].1, existing_paths);
        assert_eq!(updates[1].1, existing_paths);
        assert_eq!(updates[2].1, "VK_LAYER_OTHER:VK_LAYER_DSXINPUT_overlay");
    }

    #[test]
    fn does_not_create_vk_layer_path_when_it_was_unset() {
        let layer = LayerArtifacts {
            directory: PathBuf::from("/overlay"),
            name: OsString::from("VK_LAYER_DSXINPUT_overlay"),
        };

        let updates = layer.environment_updates(|_| None).unwrap();

        assert!(!updates.iter().any(|(key, _)| key == "VK_LAYER_PATH"));
    }
}

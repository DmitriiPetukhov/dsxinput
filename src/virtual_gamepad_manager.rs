use std::{
    collections::{HashMap, hash_map::Entry},
    ffi::{CStr, CString},
    io,
};

pub struct VirtualGamepadManager<G, F> {
    gamepads: HashMap<CString, G>,
    create_gamepad: F,
}

impl<G, F: FnMut() -> io::Result<G>> VirtualGamepadManager<G, F> {
    pub fn new(create_gamepad: F) -> Self {
        Self {
            gamepads: HashMap::new(),
            create_gamepad,
        }
    }
    pub fn create(&mut self, path: &CStr) -> io::Result<()> {
        if let Entry::Vacant(entry) = self.gamepads.entry(path.to_owned()) {
            let gamepad = (self.create_gamepad)()?;
            entry.insert(gamepad);
        }

        Ok(())
    }

    pub fn remove(&mut self, path: &CStr) {
        self.gamepads.remove(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, rc::Rc};

    fn path(value: &str) -> CString {
        CString::new(value).unwrap()
    }

    struct FakeGamepad {
        drops: Rc<Cell<usize>>,
    }

    impl Drop for FakeGamepad {
        fn drop(&mut self) {
            self.drops.set(self.drops.get() + 1);
        }
    }

    #[test]
    fn starts_empty_without_creating_devices() {
        let manager = VirtualGamepadManager::new(|| -> io::Result<()> {
            panic!("factory should not run during initialization")
        });
        assert!(manager.gamepads.is_empty());
    }

    #[test]
    fn creates_once_per_distinct_path() {
        let creations = Cell::new(0);
        let mut manager = VirtualGamepadManager::new(|| {
            creations.set(creations.get() + 1);
            Ok(creations.get())
        });
        manager.create(&path("a")).unwrap();
        manager.create(&path("a")).unwrap();
        manager.create(&path("b")).unwrap();

        assert_eq!(creations.get(), 2);
        assert_eq!(manager.gamepads.len(), 2);
        assert_eq!(manager.gamepads[&path("a")], 1);
        assert_eq!(manager.gamepads[&path("b")], 2);
    }

    #[test]
    fn failed_creation_preserves_existing_devices_and_allows_retry() {
        let attempts = Cell::new(0);
        let mut manager = VirtualGamepadManager::new(|| {
            attempts.set(attempts.get() + 1);
            if attempts.get() == 2 {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "uinput denied",
                ))
            } else {
                Ok(attempts.get())
            }
        });
        manager.create(&path("a")).unwrap();
        let error = manager.create(&path("b")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(error.to_string(), "uinput denied");
        assert_eq!(manager.gamepads.len(), 1);
        assert_eq!(manager.gamepads[&path("a")], 1);
        assert!(!manager.gamepads.contains_key(&path("b")));

        manager.create(&path("b")).unwrap();
        assert_eq!(attempts.get(), 3);
        assert_eq!(manager.gamepads[&path("b")], 3);
    }

    #[test]
    fn removal_drops_only_the_requested_device_once() {
        let drops = Rc::new(Cell::new(0));
        let mut manager = VirtualGamepadManager::new(|| {
            Ok(FakeGamepad {
                drops: Rc::clone(&drops),
            })
        });
        manager.create(&path("a")).unwrap();
        manager.create(&path("b")).unwrap();
        manager.remove(&path("unknown"));
        assert_eq!(drops.get(), 0);

        manager.remove(&path("a"));
        assert_eq!(drops.get(), 1);
        assert!(!manager.gamepads.contains_key(&path("a")));
        assert!(manager.gamepads.contains_key(&path("b")));

        manager.remove(&path("a"));
        assert_eq!(drops.get(), 1);
        drop(manager);
        assert_eq!(drops.get(), 2);
    }

    #[test]
    fn reconnect_creates_a_fresh_device() {
        let creations = Cell::new(0);
        let mut manager = VirtualGamepadManager::new(|| {
            creations.set(creations.get() + 1);
            Ok(creations.get())
        });
        manager.create(&path("a")).unwrap();
        manager.remove(&path("a"));
        manager.create(&path("a")).unwrap();

        assert_eq!(creations.get(), 2);
        assert_eq!(manager.gamepads.len(), 1);
        assert_eq!(manager.gamepads[&path("a")], 2);
    }

    #[test]
    fn dropping_manager_drops_all_remaining_devices() {
        let drops = Rc::new(Cell::new(0));
        {
            let mut manager = VirtualGamepadManager::new(|| {
                Ok(FakeGamepad {
                    drops: Rc::clone(&drops),
                })
            });
            manager.create(&path("a")).unwrap();
            manager.create(&path("b")).unwrap();
            assert_eq!(drops.get(), 0);
        }
        assert_eq!(drops.get(), 2);
    }
}

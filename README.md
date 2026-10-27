# dsxinput 0.1.1

Linux/Fedora wrapper: physical DualSense (Bluetooth/USB) evdev -> `uinput` virtual Xbox 360-style device (VID:PID `045e:028e`). Does not use Steam. Optional rumble forwarding uses Linux FF_RUMBLE: a game must actually submit `FF_RUMBLE` effects through the virtual evdev device. No HD haptics/adaptive triggers. Unit tests, lint checks, and a release build have been verified on Linux x86_64. Physical controller input and rumble still require hardware validation.

## Fedora installation

```sh
sudo dnf install rust cargo gcc evtest
sudo modprobe uinput
cd dsxinput
cargo test
cargo build --release
install -Dm755 target/release/dsxinput "$HOME/.local/bin/dsxinput"
```

Check permissions as your **normal user**:

```sh
ls -l /dev/uinput /dev/input/event*
getfacl /dev/uinput
dsxinput --diagnose
```

`--diagnose` creates an Xbox-style gamepad for ~30 seconds; `sudo evtest` in another terminal can locate and test it. Test the vibration via a rumble-capable game, or use `fftest` (from `linuxconsoletools` if available). A virtual controller **is not proof** of genuine XInput delivery by Proton; verify inside the game.

When /dev/uinput is denied, add a *narrow rule only for uinput* (recommended for local interactive sessions with logind):

```sh
printf '%s\n' 'KERNEL=="uinput", SUBSYSTEM=="misc", TAG+="uaccess"' | sudo tee /etc/udev/rules.d/70-dsxinput-uinput.rules
sudo udevadm control --reload-rules
sudo udevadm trigger --subsystem-match=misc
# Re-login if ACL is not yet applied; inspect with getfacl /dev/uinput.
```

Read access to DualSense event nodes is generally granted by the login seat (see `getfacl /dev/input/eventN`); don't use `chmod 666` or run Faugus as root. If permissions fail, inspect the actual device first.

## Usage

```sh
dsxinput --diagnose
dsxinput gamemoderun mangohud /path/to/linux-game
dsxinput /path/to/program arg1 arg2
dsxinput --no-rumble /path/to/program
dsxinput --device /dev/input/event25 /path/to/program
```

Put dsxinput options before the command. The first argument that is not a dsxinput option starts the command; every following argument is passed through unchanged. No `--` separator is required (it remains accepted for compatibility).

In Faugus native RPM installation, use `~/.local/bin/dsxinput` as the first launch wrapper (with the **absolute** home path), before `gamemoderun`, `mangohud`, etc, **only if your Faugus version's launch-arguments field actually wraps the underlying command**. Otherwise use a launcher script around `umu-run` or a Faugus supported wrapper setting. Test independently with `dsxinput sh -c 'sleep 20'` before Faugus.

Suggested game flags to test: `PROTON_PREFER_SDL=1` (if HIDRAW picks up the original controller), or toggle Faugus `Disable Hidraw`, **but test per game**. `EVIOCGRAB` hides evdev input from other evdev clients; it does NOT hide hidraw or prevent every possible duplicate.

## Constraints

- Single controller at a time; automatically detects Sony DualSense product IDs 054c:0ce6 and 054c:0df2 with gamepad axes.
- Standard digital buttons, two thumbsticks, hat/d-pad, and two analog triggers. Touchpad/gyro/audio/haptics not supported.
- For physical rumble, the kernel driver needs `FF_RUMBLE`, the input event node needs write permission, and Wine/Proton must send FF effects to the virtual evdev device.
- Child command/process-tree lifetime varies with games and launchers. This wrapper waits for **its direct child**, not necessarily every detached Wine subprocess. Terminating wrapper sends SIGTERM to the child's process group, not every possible detached process.
- Bluetooth disconnect currently causes wrapper to error, potentially leaving the launched game running; the virtual controller is then destroyed on exit.
- Device may be visible twice via HIDRAW. There is no safe universal per-game isolation implemented here.
- No restart after Bluetooth reconnect. No multi-game concurrent access; `EVIOCGRAB` is exclusive for the selected evdev device.

## Development

`main.rs` is the entry point; `app.rs` runs the event loop. The remaining modules isolate command parsing (`cli`), event translation (`mapping`), device setup (`device`), rumble forwarding (`feedback`), game launch (`process`), and the Linux ABI (`linux`).

Use test-first changes and keep each refactoring step passing:

```sh
cargo test --locked
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo build --release --locked
```

The tests run without controller permissions. They cover parsing, event mapping and scaling, ABI layouts, event I/O, rumble playback routing, controller identity, and child process launch. Successful kernel rumble upload/erase and controller discovery still need hardware tests.

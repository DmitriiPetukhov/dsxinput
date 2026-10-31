# dsxinput — DualSense to XInput for Linux

Use your PS5 DualSense controller in Linux games that expect an Xbox 360 controller—without Steam or Steam Input.

Wrap your game command with `dsxinput`. Controller emulation runs only while the game is running. Supports USB, Bluetooth, and standard rumble.

Valve's Steam Input makes this easy: it can emulate an XInput controller for games that don't support DualSense directly. But for games bought elsewhere and run through other launchers, that usually means adding them to Steam first and enabling Steam Input there. I wanted something simpler—a command you can add to a game's launch settings, with controller emulation running only for that game.

That's what dsxinput does. It wraps the game command, creates a virtual Xbox 360-style controller, and removes it when the command exits. No Steam required.

## Installation (Fedora)

### Build requirements and device access

Build with Rust 1.95 or newer and Cargo. Check the installed versions with `rustc --version` and `cargo --version`. A C compiler is required for native dependencies, and `curl` is used by the rustup installer:

```sh
sudo dnf install gcc curl
```

If you do not have a Rust toolchain at version 1.95 or newer, install [rustup](https://rust-lang.org/tools/install/) and select the stable toolchain:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
. "$HOME/.cargo/env"
rustup toolchain install stable
rustup default stable
rustc --version
```

Confirm that `rustc --version` reports 1.95 or newer before building.

The Vulkan overlay also needs the system Vulkan loader and a working graphics driver. Install `vkcube` if you want to run the overlay smoke check.

Enable `uinput` and allow your local desktop session to access it:

```sh
sudo modprobe uinput
printf '%s\n' uinput | sudo tee /etc/modules-load.d/dsxinput.conf
printf '%s\n' 'KERNEL=="uinput", SUBSYSTEM=="misc", TAG+="uaccess"' \
  | sudo tee /etc/udev/rules.d/70-dsxinput-uinput.rules
sudo udevadm control --reload-rules
sudo udevadm trigger --subsystem-match=misc
```

Log out and back in if the permission change does not take effect. Your user also needs read/write access to the DualSense input device under `/dev/input/event*`.

### Build and install

From the project directory:

```sh
cargo build --workspace --release --locked
./install.sh
```

The installer puts `dsxinput` in `~/.local/bin` and the Vulkan layer library and manifest in `~/.local/lib/dsxinput`. Set `PREFIX` to install under another prefix:

```sh
PREFIX="$HOME/.local-alt" ./install.sh
```

Make sure the selected prefix's `bin` directory is in your `PATH` to use the commands below. You can also run the installer harness with `sh tests/install.sh`.

## Usage

### On its own

Launch a game with or without a DualSense connected:

```sh
dsxinput /path/to/game --fullscreen
```

Arguments after the game command are passed through unchanged. You can connect your DualSense over USB or Bluetooth before or during the game.

### Show the Vulkan version overlay

Pass `--debug` before the game command to enable the version overlay for that child process:

```sh
dsxinput --debug vkcube
dsxinput --debug /path/to/game --fullscreen
```

The first command is a smoke check when `vkcube` is installed. The overlay is loaded only for the launched game; a normal `dsxinput GAME` launch does not enable it.

### With GameMode and MangoHud

With both tools installed:

```sh
dsxinput --debug gamemoderun mangohud /path/to/game
```

### In Faugus Launcher

For a native installation of [Faugus Launcher](https://github.com/Faugus/faugus-launcher), edit the game's **Launch Arguments** field and enter the absolute path to dsxinput:

```text
/home/your-user/.local/bin/dsxinput --debug
```

Replace `/home/your-user` with your home directory. Faugus adds the game command automatically; no `%command%` placeholder is needed. Remove `--debug` if you do not want the Vulkan overlay. You can enable GameMode and MangoHud through Faugus as usual—they run after dsxinput.

### Vulkan overlay support

The layer targets Linux x86_64, including 64-bit native Vulkan games and Wine/Proton games using DXVK or VKD3D-Proton. DXVK/VKD3D compatibility is a target and has not yet been verified with real games. `vkcube` smoke checks confirm the layer can draw on this system, but do not establish compatibility with a particular game.

The layer applies only to Vulkan. It supports single-layer SDR color-attachment swapchains where graphics and presentation use the same queue family. OpenGL games do not use the layer; 32-bit processes and Flatpak installs are unsupported. The layer skips unsupported swapchain modes such as HDR or separate graphics/presentation queue families and logs a diagnostic.

## How it works

dsxinput creates a virtual Xbox 360-style controller before launching the game and forwards DualSense input and standard rumble. You can connect or reconnect the DualSense during play: the game keeps the same virtual controller, and disconnecting clears held input. Connection or setup errors are reported without stopping the game.

When the game command exits, dsxinput removes the virtual controller and releases the DualSense. SIGINT or SIGTERM also stops the launched process group.

Only one controller is supported at a time. Adaptive triggers, advanced haptics, touchpad, and gyro are unsupported. Some games can also detect the physical DualSense and may need their hidraw access disabled to avoid duplicate input.

## Troubleshooting

### The game does not detect the controller

Run `dsxinput --diagnose` to check detection and event forwarding without launching a game. Check the reported errors: your user needs read/write access to the DualSense input device and access to `/dev/uinput`, and the controller must support physical rumble.

The virtual controller should exist before the game starts. Check the dsxinput output for virtual controller setup errors. If you use `--device`, check that the path still points to the controller after reconnecting, or omit it to enable automatic discovery.

### Permission denied for /dev/uinput

Follow the `uinput` setup steps under [Build requirements and device access](#build-requirements-and-device-access). Log out and back in if the permission change does not take effect.

### The game receives duplicate input

Some games can read the physical DualSense through hidraw alongside the virtual Xbox controller. Disable the game's hidraw controller access if it provides that option.

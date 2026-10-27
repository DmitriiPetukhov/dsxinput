# dsxinput — DualSense to XInput for Linux

Use your PS5 DualSense controller in Linux games that expect an Xbox 360 controller—without Steam or Steam Input.

Wrap your game command with `dsxinput`. Controller emulation runs only while the game is running. Supports USB, Bluetooth, and standard rumble.

Valve's Steam Input makes this easy: it can emulate an XInput controller for games that don't support DualSense directly. But for games bought elsewhere and run through other launchers, that usually means adding them to Steam first and enabling Steam Input there. I wanted something simpler—a command you can add to a game's launch settings, with controller emulation running only for that game.

That's what dsxinput does. It wraps the game command, creates a virtual Xbox 360-style controller, and removes it when the command exits. No Steam required.

## Installation (Fedora)

### Dependencies and device access

Install the build tools:

```sh
sudo dnf install rust cargo gcc
```

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
cargo build --release --locked
install -Dm755 target/release/dsxinput "$HOME/.local/bin/dsxinput"
```

Make sure `~/.local/bin` is in your `PATH` to use the commands below.

## Usage

### On its own

Launch a game with or without a DualSense connected:

```sh
dsxinput /path/to/game --fullscreen
```

Arguments after the game command are passed through unchanged. You can connect your DualSense over USB or Bluetooth before or during the game.

### With GameMode and MangoHud

With both tools installed:

```sh
dsxinput gamemoderun mangohud /path/to/game
```

### In Faugus Launcher

For a native installation of [Faugus Launcher](https://github.com/Faugus/faugus-launcher), edit the game's **Launch Arguments** field and enter the absolute path to dsxinput:

```text
/home/your-user/.local/bin/dsxinput
```

Replace `/home/your-user` with your home directory. Faugus adds the game command automatically; no `%command%` placeholder is needed. You can enable GameMode and MangoHud through Faugus as usual—they run after dsxinput.

## How it works

dsxinput launches the game even when no controller is connected. When it detects an accessible DualSense, it takes exclusive access to its evdev input and creates a virtual Xbox 360-style controller through Linux's `uinput` interface. It translates buttons, sticks, the D-pad, and triggers, and forwards standard rumble effects back to the DualSense.

If the controller disconnects, dsxinput removes the virtual controller and keeps the game running. While disconnected, it searches for a controller every 500 ms. Connecting or reconnecting creates a fresh virtual controller with the current button and axis state. The game must support controller hotplug to use a controller added during play.

Device access or setup errors are reported without stopping the game; dsxinput keeps retrying. Read/write input access, `uinput` access, and physical rumble support are required for controller emulation.

To select a specific input node, use `dsxinput --device /dev/input/eventN COMMAND [ARGS...]`. This retries only the specified path. Event numbers can change after reconnecting; omit `--device` for automatic discovery, or use a stable device symlink.

When the launched command exits, dsxinput removes the virtual controller and releases the physical one. SIGINT or SIGTERM also stops the launched process group. `dsxinput --diagnose` runs detection and event forwarding for 30 seconds without launching a game, including when no controller is connected.

Only one controller is handled at a time. DualSense-specific features such as adaptive triggers, advanced haptics, touchpad, and gyro are not supported. The evdev grab does not hide the original controller from hidraw, so some games may need their hidraw access disabled to avoid duplicate input.

## Troubleshooting

### The game does not detect the controller

Run `dsxinput --diagnose` to check detection and event forwarding without launching a game. Check the reported errors: your user needs read/write access to the DualSense input device and access to `/dev/uinput`, and the controller must support physical rumble.

If you connected the controller after starting the game, try connecting it before launch. Some games do not support controller hotplug. If you use `--device`, check that the path still points to the controller after reconnecting, or omit it to enable automatic discovery.

### Permission denied for /dev/uinput

Follow the `uinput` setup steps under [Dependencies and device access](#dependencies-and-device-access). Log out and back in if the permission change does not take effect.

### The game receives duplicate input

Some games can read the physical DualSense through hidraw alongside the virtual Xbox controller. Disable the game's hidraw controller access if it provides that option.

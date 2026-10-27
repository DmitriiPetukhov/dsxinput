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

dsxinput creates a virtual Xbox 360-style controller before launching the game and forwards DualSense input and standard rumble. You can connect or reconnect the DualSense during play: the game keeps the same virtual controller, and disconnecting clears held input. Connection or setup errors are reported without stopping the game.

When the game command exits, dsxinput removes the virtual controller and releases the DualSense. SIGINT or SIGTERM also stops the launched process group.

Only one controller is supported at a time. Adaptive triggers, advanced haptics, touchpad, and gyro are unsupported. Some games can also detect the physical DualSense and may need their hidraw access disabled to avoid duplicate input.

## Troubleshooting

### The game does not detect the controller

Run `dsxinput --diagnose` to check detection and event forwarding without launching a game. Check the reported errors: your user needs read/write access to the DualSense input device and access to `/dev/uinput`, and the controller must support physical rumble.

The virtual controller should exist before the game starts. Check the dsxinput output for virtual controller setup errors. If you use `--device`, check that the path still points to the controller after reconnecting, or omit it to enable automatic discovery.

### Permission denied for /dev/uinput

Follow the `uinput` setup steps under [Dependencies and device access](#dependencies-and-device-access). Log out and back in if the permission change does not take effect.

### The game receives duplicate input

Some games can read the physical DualSense through hidraw alongside the virtual Xbox controller. Disable the game's hidraw controller access if it provides that option.

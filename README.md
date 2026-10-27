# dsxinput

A small utility for using DualSense controllers in Linux games.

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

Connect your DualSense over USB or Bluetooth, then launch a game:

```sh
dsxinput /path/to/game --fullscreen
```

Arguments after the game command are passed through unchanged.

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

Before launching the game, dsxinput detects a DualSense controller, takes exclusive access to its evdev input, and creates a virtual Xbox 360-style controller through Linux's `uinput` interface. It translates buttons, sticks, the D-pad, and triggers, and forwards standard rumble effects back to the DualSense.

When the launched command exits, dsxinput removes the virtual controller and releases the physical one. If the controller disconnects, it shuts down the launched process group as well.

Only one controller is handled at a time. DualSense-specific features such as adaptive triggers, advanced haptics, touchpad, and gyro are not supported. The evdev grab does not hide the original controller from hidraw, so some games may need their hidraw access disabled to avoid duplicate input.

# dsxinput

Use DualSense and DualSense Edge controllers as virtual Xbox 360-style gamepads on Linux, without Steam Input. Supports multiple controllers over USB or Bluetooth.

Valve's Steam Input makes this easy: it can emulate an XInput controller for games that don't support DualSense directly. But for games bought elsewhere and run through other launchers, that usually means adding them to Steam first and enabling Steam Input there. I wanted something simpler—a command you can add to a game's launch settings, with controller emulation running only for that game.

That's what dsxinput does. It wraps the game command, creates a virtual Xbox 360-style controller, and removes it when the command exits. No Steam required.

## Install (Fedora)

Install build dependencies, then run the installer from the project directory:

```sh
sudo dnf install rust cargo gcc pkgconf-pkg-config systemd-devel
./install.sh
```

The installer builds a release binary and copies it to `~/.local/bin/dsxinput`. Add `~/.local/bin` to your `PATH`. To choose another location, run `PREFIX=/your/prefix ./install.sh`.

## Usage

Launch a game with its arguments:

```sh
dsxinput /path/to/game --fullscreen
```

Controllers can connect before or during play. When the launched process exits, dsxinput exits and removes its virtual gamepads. Use a command that stays running for the game's lifetime.

Or run without launching a game:

```sh
dsxinput --standalone
```

Ctrl+C stops dsxinput and removes its virtual gamepads while leaving the launched game running.

To run from source, use `cargo run --` followed by the same arguments.

### Faugus Launcher

Put the absolute path to dsxinput in the game's **Launch Arguments** field:

```text
/home/your-user/.local/bin/dsxinput
```

Faugus supplies the game command automatically. No `%command%` or extra `--` is needed.

### Steam

For games launched through Steam, use Steam Input instead of dsxinput. Enable Steam Input in the game's controller settings.

## Configuration

Configure stick deadzones through environment variables:

| Variable | Description | Default |
|---|---|---|
| `DSXINPUT_STICK_DEADZONE_L` | Left stick axial deadzone | `0.1` (10%) |
| `DSXINPUT_STICK_DEADZONE_R` | Right stick axial deadzone | `0.1` (10%) |

Values must be finite numbers from `0` (inclusive) to `1` (exclusive). Invalid values stop startup.

For example:

```sh
DSXINPUT_STICK_DEADZONE_L=0.05 DSXINPUT_STICK_DEADZONE_R=0.15 dsxinput --standalone
```

## Limitations

- Rumble, adaptive triggers, advanced haptics, touchpad, and gyro are not supported.
- Physical controllers remain visible to games, which can cause duplicate input.
- Session failures are logged; reconnect the controller to retry. Input frames are currently printed for debugging.

## Device permissions

If a controller is missing or virtual device creation fails, run `dsxinput --standalone` and check its output. Your user needs read access to the controller's `/dev/input/event*` node and write access to `/dev/uinput`.

Skip the following setup if virtual gamepads already work. Otherwise, enable `uinput` and grant access to the active local desktop session:

```sh
sudo modprobe uinput
printf '%s\n' uinput | sudo tee /etc/modules-load.d/dsxinput.conf
printf '%s\n' 'KERNEL=="uinput", SUBSYSTEM=="misc", TAG+="uaccess"' \
  | sudo tee /etc/udev/rules.d/70-dsxinput-uinput.rules
sudo udevadm control --reload-rules
sudo udevadm trigger --subsystem-match=misc
```

Your user also needs read access to the physical controller's `/dev/input/event*` gamepad node. Log out and back in if device permission changes do not take effect.

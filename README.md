# BrightnessControl

## Disclaimer: Linux + Xorg Only

[![Build Status](https://travis-ci.org/srithon/BrightnessControl.svg?branch=master)](https://travis-ci.org/srithon/BrightnessControl)

`BrightnessControl` is a wrapper around `xrandr` that allows for easy adjustments of brightness.

This brightness is separate from the backlight.

It also allows for an emulation of a blue light filter / night light, which can be toggled on/off. This emulation is part of `xrandr` itself.

To use `redshift` instead of `xrandr` for the blue light filter, set `use_redshift` to `true` in the configuration file. More details on this will be in the `Configuration` section

***

Since version `1.3.0`, `BrightnessControl` uses a daemon to interface with `xrandr`, and client instances to interface with the daemon.

When the daemon is started, it loads the following values from disk
* brightness: [0..100] percentage of full brightness
  * stored in `~/.cache/brightnesscontrol/brightness`
* mode: 0 or 1; 0 means nightlight is off, 1 means it is on
  * stored in `~/.cache/brightnesscontrol/mode`
* displays: list of connected display adapters
  * stored in `~/.cache/brightnesscontrol/displays`

If the contents of either `brightness` or `mode` are invalid, they are automatically defaulted and overwritten

If the `displays` file is empty or non-existent, it will automatically be populated with the current display configuration

After starting, the daemon stores all of these values in memory, and does not touch the files again until it receives a `SIGTERM` signal.

Upon receiving this signal, the daemon writes all of the new values to the filesystem and terminates.

Manually modifying these files while the daemon is running will have no effect.

***

If the daemon's call to `xrandr` *fails* as a result of invalid/outdated data in its in-memory `displays` field, the program will automatically reconfigure its list of displays **IF** `auto-reconfigure` is set to `true` in the configuration file.

When this is not enabled, each individual client message takes less time to process because the daemon does not have to wait for each `xrandr` call to terminate before moving onto the next one

The `xrandr` call will only fail if a display is *disconnected*, so even with `auto-reconfigure` enabled, the daemon will not automatically reconfigure when a new monitor is connected.

For users that often disconnect and reconnect monitors, the [srandrd](https://github.com/jceb/srandrd) tool can be used to automatically call `brightness_control --configure-display` whenever the monitor setup changes

This is a good alternative to `auto-reconfigure`, which also has the advantage of working when a new monitor is connected

## Installation
*From the project root*

```
cargo install --path . --root ~/.local/
```

***

`cargo` will append `bin/` to the end of the path that you pass in for `--root`, so the above command will install the executable into `~/.local/bin/`

### Configuring Redshift
*Change redshift's adjustment mode*
```
sed -i 's/adjustment-method=randr/adjustment-method=vidmode' ~/.config/redshift.conf
```

## Usage
*All examples assume that the name of the executable is `brightness_control` and that the executable can be found in one of the directories in the `PATH` environmental variable*

All brightness_control options have shorthands. For most of them, the first letter of the option's name acts as the shorthand. However, "daemon" and "decrement" both start with 'd'; "decrement" came first, so "daemon"'s shorthand is "-b"

When using shorthands, the separating space between an option and its value may be omitted. This is shown in the first example, and can be applied to any option that takes a value

*To Reduce the Brightness by 10%*
```
$ brightness_control --decrement 10
```
or
```
$ brightness_control -d10
```

*To Increase the Brightness by 10%*
```
$ brightness_control --increment 10
```

*To Set the Brightness to 80%*
```
$ brightness_control --set 80
```

*To Refresh the Current Brightness*
```
$ brightness_control
```

*To Toggle Night Light*
```
$ brightness_control --toggle-nightlight
```

*To Reconfigure the Cached Display Settings*

This is necessary when a new display adapter is connected
```
$ brightness_control --configure-display
```

*To Start the Daemon*
```
$ brightness_control --daemon
```

*To View the Help Menu*
```
$ brightness_control --help
```

*To Reload the Daemon Configuration*
```
$ brightness_control --reload-configuration
```

## Configuration
`BrightnessControl`'s reads its runtime configuration from `~/.config/brightnesscontrol/config.toml`
If the file does not exist when the daemon is started, it automatically creates the file and writes the default configuration.

After that, you can reload the configuration without restarting the daemon by running `brightness_control --reload-configuration` as shown above.

If the file cannot be parsed, the client will print out the error.

## Keybinding
All of these commands can be bound to keybindings for ease-of-use.

In the future, this functionality may be integrated directly into the application, but for now this can be done with [xbindkeys](https://wiki.archlinux.org/index.php/Xbindkeys) or other similar utilities.

An example schema is below

Alt+PgUp        -> `brightness_control -i10`

Alt+PgDown      -> `brightness_control -d10`

Alt+Ctrl+PgUp   -> `brightness_control -i2`

Alt+Ctrl+PgDown -> `brightness_control -d2`

Alt+End         -> `brightness_control -t`

Alt+Home        -> `brightness_control -c`


# BrightnessControl

## Disclaimer: Linux + Xorg Only

[![Build Status](https://travis-ci.org/srithon/BrightnessControl.svg?branch=master)](https://travis-ci.org/srithon/BrightnessControl)

`BrightnessControl` is a wrapper around `xrandr` that allows for easy adjustments of brightness.

This brightness is separate from the backlight.

It also allows for an emulation of a blue light filter / night light, which can be toggled on/off. This emulation is part of `xrandr` itself.

To use `redshift` instead of `xrandr` for the blue light filter, set `use_redshift` to `true` in the configuration file. More details on this will be in the `Configuration` section

Since version `1.4.5`, `BrightnessControl` can smoothly fade between brightness levels. More information on brightness fading can be found in the configuration template.

Since version `1.6.0`, `BrightnessControl` processes input asynchronously, **and multiple monitors function as expected**

***

Since version `1.3.0`, `BrightnessControl` uses a daemon to interface with `xrandr`, and client instances to interface with the daemon.

When the daemon is started, it loads the following values from disk
* stored in `~/.cache/brightnesscontrol/persistent_state.toml`
  * brightness: [0..100] percentage of full brightness
  * nightlight: `true` or `false`; `false` means nightlight is off, `true` means it is on

If the values cannot be parsed correctly, the daemon will instead use the default values.

After starting, the daemon stores all of these values in memory, and does not touch the files again until it receives a `SIGTERM` signal.

Upon receiving this signal, the daemon writes all of the new values to the filesystem and terminates.

Manually modifying these files while the daemon is running will have no effect.

***

If the daemon's call to `xrandr` *fails* as a result of invalid/outdated data in its in-memory `displays` field, the program will automatically remove the corresponding display from the list **IF** `auto_remove_displays` is set to `true` in the configuration file.

When this is not enabled, each individual client message takes less time to process because the daemon does not have to wait for each `xrandr` call to terminate before moving onto the next one

If your display configuration is mostly static, consider disabling this option.

For users that often disconnect and reconnect monitors, two external options are the [autorandr](https://github.com/phillipberndt/autorandr) and [srandrd](https://github.com/jceb/srandrd) programs.

Both programs can be used to automatically call `brightness_control --configure-display` whenever the monitor setup changes

This is a good alternative to `auto_remove_displays`, which also has the advantage of working when a _new_ monitor is connected

Instructions on how to configure these programs to work with this application will be added in the future.

Instructions on how to enable/disable `auto_remove_displays` are in the Installation section

## Build Dependencies
**NOTE**: since `v1.4.1`, you can download pre-compiled binaries from [GitHub releases](https://github.com/srithon/BrightnessControl/releases). These binaries are packaged by Travis CI. If you do not want to download `Rust` (admittedly a pretty big dependency), you should consider downloading and using these binaries.

1) `cargo`

The easiest way to install `cargo` is through `rustup`: the Rust toolchain installer.

The following command is taken directly from the [cargo installation page](https://doc.rust-lang.org/cargo/getting-started/installation.html).

It downloads the `rustup` installation script and pipes it into `sh`, which executes it directly.

`$ curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`

To read the script before it executes, you can redirect the output into a file by replacing `| sh` with `> filename.sh`. After reading `filename.sh`, you can execute it from there.

**Alternatively**, you can download the `rustup` package from your distro's (most likely) official repositories.

**Ubuntu**: `sudo apt install rustup`

**Arch Linux**: `sudo pacman -S rustup`

_With rustup installed_...
```
$ rustup toolchain install stable
$ rustup default stable
```

This should install `cargo` for you.

## Runtime Dependencies
1) `xrandr`

`xrandr` may already be installed on your computer. Type `xrandr` into your shell to check.

**Ubuntu**: `sudo apt install x11-xserver-utils `

**Arch Linux**: `sudo pacman -S xorg-xrandr`

2) `redshift` (optional)

**Ubuntu**: `sudo apt install redshift`

**Arch Linux**: `sudo pacman -S redshift`

Source: [https://github.com/jonls/redshift](https://github.com/jonls/redshift)

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
sed -i 's/adjustment-method=randr/adjustment-method=vidmode/' ~/.config/redshift.conf
```

## Usage
*All examples assume that the name of the executable is `brightness_control` and that the executable can be found in one of the directories in the `PATH` environmental variable*

All brightness_control options have shorthands. For most of them, the first letter of the option's name acts as the shorthand.

When using shorthands, the separating space between an option and its value may be omitted. This is shown in the first example, and can be applied to any option that takes a value

*To Reduce the Brightness by 10%*
```
$ brightness_control brightness --decrement 10
```
or
```
$ brightness_control b -d10
```

*To Increase the Brightness by 10%*
```
$ brightness_control b --increment 10
```

*To Set the Brightness to 80%*
```
$ brightness_control b --set 80
```

_To Set the Brightness to 50% Without Fading_
```
$ brightness_control b -ns80
```

_To Terminate the Current Brightness Fade_
```
$ brightness_control b -t
```

_To Terminate/Interrupt the Current Brightness Fade and Decrement the Brightness by 10%_
```
$ brightness_control b -td10
```

*To Toggle Night Light*
```
$ brightness_control nightlight --toggle
```

*To Reconfigure the Cached Display Settings*

This is necessary when a new display adapter is connected
```
$ brightness_control --configure-display
```

*To Start the Daemon*
```
$ brightness_control daemon --start
```

*To View the Help Menu*
```
$ brightness_control --help
```

_To Print Out the Current Configuration Template_
```
$ brightness_control config --print-config-template
```

_To Print Out the Current Brightness_
```
$ brightness_control --get b
```

_To Print Out the Currently Loaded Configuration_
```
$ brightness_control --gconfiguration
```

*To Reload the Daemon Configuration*
```
$ brightness_control c --reload
```

## Configuration
`BrightnessControl`'s reads its runtime configuration from `~/.config/brightnesscontrol/config.toml`
If the file does not exist when the daemon is started, it automatically creates the file and writes the default configuration.

The default configuration template is stored in `~/.local/share/brightnesscontrol/config_template.toml`

The daemon writes the template to disk when it is first started. To do this manually, copy the output of `brightness_control c --print-default` to the file.

Whenever the daemon is started, it checks to see if the configuration template is out of date. If it is, then it overwrites it with the current template.

When the daemon overwrites the template, it will indicate this through stdout. `BrightnessControl` will never overwrite your personal configuration, so whenever new options are added to the template, they have to be copied over manually.

You can reload the configuration without restarting the daemon by running `brightness_control --reload` as shown above.

If the file cannot be parsed, the client will print out the error.

All available options are documented in the template

## Keybinding
All of these commands can be bound to keybindings for ease-of-use.

In the future, this functionality may be integrated directly into the application, but for now this can be done with [xbindkeys](https://wiki.archlinux.org/index.php/Xbindkeys) or other similar utilities.

An example schema is below

Alt+PgUp        -> `brightness_control b -i10`

Alt+PgDown      -> `brightness_control b -d10`

Alt+Ctrl+PgUp   -> `brightness_control b -i2`

Alt+Ctrl+PgDown -> `brightness_control b -d2`

Alt+End         -> `brightness_control n -t`

Alt+Home        -> `brightness_control -c`


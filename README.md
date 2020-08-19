# BrightnessControl

## Disclaimer: Linux + Xorg Only

`BrightnessControl` is a wrapper around `xrandr` that allows for easy adjustments of brightness.

This brightness is separate from the backlight.

It also allows for an emulation of a blue light filter / night light, which can be toggled on/off. This emulation is part of `xrandr` itself.

To use `redshift` instead of `xrandr` for the blue light filter, pass in the `redshift` feature during installation. Details on how to do this will be in the `Installation` section

Internally, the brightness is stored in `~/.cache/brightnesscontrol/brightness`

It is a number between 0 and 100 (inclusive) which represents the current brightness relative to max brightness

The mode (nightlight or normal) is stored in `~/.cache/brightnesscontrol/mode`

It is either `0` or `1`

`0`: nightlight is off

`1`: nightlight is on

If the file contents are invalid, they are automatically defaulted and overwritten for both `brightness` and `mode`

The displays that BrightnessControl will supply to its `xrandr` calls are stored in `~/.cache/brightnesscontrol/displays`

If the executable is called and the `displays` file is empty or non-existent, it will automatically populate the file with the current display configuration

If the `xrandr` call *fails* as a result of invalid/outdated data in the cache, the program will automatically reconfigure the display **IF** the `auto-reconfigure` feature is enabled at compile-time.

When this feature is not enabled, the program takes less time to run because it does not have to wait for the `xrandr` call to terminate before terminating itself

However, the runtime difference will be negligible for most users.

The following benchmarks were run using [hyperfine](https://github.com/sharkdp/hyperfine)

**With auto-reconfigure enabled**
```
Benchmark #0: ~/.local/bin/brightness_control + 0.01
  Time (mean ± σ):      28.7 ms ±   4.8 ms    [User: 9.6 ms, System: 6.3 ms]
  Range (min … max):    13.5 ms …  32.1 ms    90 runs
```

**Without auto-reconfigure enabled**
```
Benchmark #1: ~/.local/bin/brightness_control + 0.01
  Time (mean ± σ):       0.9 ms ±   0.4 ms    [User: 0.9 ms, System: 1.0 ms]
  Range (min … max):     0.5 ms …   2.6 ms    90 runs
```

Instructions on how to enable/disable this feature are in the Installation section

_General Notes_
* If you modify any of these files from outside the program, run the executable with no arguments to make the changes active
* All 3 of these files are automatically created and populated if they do not exist
  * if `brightness` or `mode` contain invalid data, they will be defaulted and overwritten
  * if `displays` contains invalid data and causes the `xrandr` call to fail, it will be overwritten if the `auto-reconfigure` feature is enabled
    * this can happen if an adapter is disconnected after the configuration is made
      * **however, if the reverse happens and a display is added after the configuration is made, auto-reconfigure will not reconfigure**
        * instructions on how to manually reconfigure are in the `Usage` section
    * *if you are going to be changing your display configuration often, you should probably use the feature*

## Installation
*From the project root*

The list of features that are enabled by default are under the `[features]` section in `Cargo.toml`
Currently, there are no features enabled by default; this is denoted by the following line
`default = []`

**WITH DEFAULT FEATURES**
```
cargo install --path . --root ~/.local/
```

**WITH OPTIONAL FEATURES**

During installation, pass in a space-separated list of the features that you want after `--features`

The list of available features can be found in the `[features]` section in `Cargo.toml`

The resulting command is below; anything in square brackets is optional
```
cargo install --features [auto-reconfigure] [redshift] --path . --root ~/.local/
```

***

`cargo` will append `bin/` to the end of the path that you pass in for `--root`, so the above command will install the executable into `~/.local/bin/`

This is because `brightness_control.rs` is inside of a `bin` subdirectory

### Configuring Redshift
*Change redshift's adjustment mode*
```
sed -i 's/adjustment-method=randr/adjustment-method=vidmode' ~/.config/redshift.conf
```

## Usage
*All examples assume that the name of the executable is `brightness_control` and that the executable can be found in one of the directories in the `PATH` environmental variable*

*To Reduce the Brightness by 10%*
```
brightness_control -10
```

*To Increase the Brightness by 10%*
```
brightness_control 10
```

*To Refresh the Current Brightness*
```
brightness_control
```

*To Toggle Night Light*
```
brightness_control --toggle
```

*To Reconfigure the Cached Display Settings*

This is necessary when a new display adapter is connected
```
brightness_control --configure-display
```

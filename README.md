# BrightnessControl

## Disclaimer: Linux + Xorg Only

`BrightnessControl` is a wrapper around `xrandr` that allows for easy adjustments of brightness.

This brightness is separate from the backlight.

It also allows for an emulation of a blue light filter / night light, which can be toggled on/off. For a true blue light filter, look at the `redshift` branch of this project


Internally, the brightness is stored in `~/.cache/brightnesscontrol/brightness`

It is a number between 0 and 100 (inclusive) which represents the current brightness relative to max brightness

The mode (nightlight or normal) is stored in `~/.cache/brightnesscontrol/mode`

It is either `0` or `1`

`0`: nightlight is off

`1`: nightlight is on


If you modify either of these files from outside the program, run the executable with no arguments to make the changes active

These files are automatically created if they do not exist

Also, if the file contents are invalid, they are automatically defaulted and overwritten

## Installation
*From the project root*
```
cargo install --path . --root ~/.local/
```
`cargo` will append `bin/` to the end of the path that you pass in for `--root`, so the above command will install the executable into `~/.local/bin/`

This is because `brightness_control.rs` is inside of a `bin` subdirectory

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

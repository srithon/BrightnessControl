# BrightnessControl

A command-line tool for adjusting screen brightness on Linux systems using Xorg. Unlike traditional backlight controls, BrightnessControl uses `xrandr` to modify display brightness at the software level, giving you fine-grained control over your monitors.

## Features

- **Per-monitor brightness control** - Set different brightness levels for each connected display
- **Smooth brightness transitions** - Gradual fading between brightness levels
- **Blue light filter** - Built-in night light functionality using `xrandr` or `redshift`
- **Multi-monitor support** - Control all monitors simultaneously or target specific displays
- **Daemon architecture** - Fast, responsive brightness adjustments with persistent settings

## Requirements

- Linux with Xorg (Wayland not supported)
- `xrandr` (usually pre-installed)
- `redshift` (optional, for enhanced blue light filtering)

## Installation

### Option 1: Pre-compiled Binaries (Recommended)

Download the latest binary from [GitHub Releases](https://github.com/srithon/BrightnessControl/releases) and place it in your PATH.

### Option 2: Build from Source

1. Install Rust:
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

2. Install runtime dependencies:
```bash
# Ubuntu/Debian
sudo apt install x11-xserver-utils redshift

# Arch Linux
sudo pacman -S xorg-xrandr redshift
```

3. Build and install:
```bash
cargo install --path . --root ~/.local/
```

Make sure `~/.local/bin` is in your PATH.

## Quick Start

1. **Start the daemon:**
```bash
brightness_control daemon --start
```

2. **Adjust brightness:**
```bash
# Increase brightness by 10%
brightness_control brightness --increment 10

# Set brightness to 50%
brightness_control brightness --set 50

# Decrease brightness by 5%
brightness_control brightness --decrement 5
```

3. **Toggle night light:**
```bash
brightness_control nightlight --toggle
```

## Usage Examples

### Basic Brightness Control

```bash
# Using full commands
brightness_control brightness --increment 10
brightness_control brightness --set 80

# Using shortcuts (same functionality)
brightness_control b -i10
brightness_control b -s80
```

### Multi-Monitor Control

```bash
# Control all connected monitors
brightness_control --enabled brightness --set 70

# Control specific monitor
brightness_control --monitor eDP-1 brightness --increment 15

# Control the "active" monitor (BrightnessControl's concept)
brightness_control --active brightness --decrement 5
```

### Advanced Features

```bash
# Set brightness without smooth fading
brightness_control b --no-fade --set 60

# Interrupt current fade and change brightness
brightness_control b --terminate-fade --increment 10

# Get current brightness levels
brightness_control get --brightness

# List connected displays
brightness_control get --displays
```

### Monitor Management

```bash
# Refresh display configuration (after connecting/disconnecting monitors)
brightness_control monitors --reconfigure-displays

# Set active monitor for --active flag
brightness_control monitors --set-active HDMI-1
```

## Configuration

BrightnessControl stores its configuration in `~/.config/brightnesscontrol/config.toml`. The daemon creates this file with default settings on first run.

To see all available options:
```bash
brightness_control config --print-config-template
```

To reload configuration without restarting:
```bash
brightness_control config --reload
```

### Key Configuration Options

- **Brightness fading**: Control smooth transitions between brightness levels
- **Blue light filter**: Choose between `xrandr` (default) or `redshift`
- **Auto-remove displays**: Automatically handle disconnected monitors
- **Default monitor behavior**: Set how commands apply to monitors by default

### Configuring Redshift

To allow `brightness_control` to use `redshift` as a blue light filter while still using `xrandr` for brightness control, you must change redshift's adjustment mode away from `randr` to `vidmode`.
```bash
sed -i 's/adjustment-method=randr/adjustment-method=vidmode/' ~/.config/redshift.conf
```

## Keybinding Setup

For convenient daily use, bind these commands to keyboard shortcuts using your desktop environment's settings or tools like `xbindkeys`:

```bash
# Example keybindings
Alt+PageUp     → brightness_control b -i10    # Increase brightness
Alt+PageDown   → brightness_control b -d10    # Decrease brightness
Alt+End        → brightness_control n -t      # Toggle night light
Alt+Home       → brightness_control m -r      # Refresh displays
```

## Troubleshooting

**Daemon won't start**: Check if another instance is running with `killall brightness_control`

**No brightness change**: Ensure `xrandr` works directly and your display supports software brightness control

**Monitor not detected**: Run `brightness_control monitors --reconfigure-displays` after connecting new displays

## How It Works

BrightnessControl uses a client-daemon architecture. The daemon interfaces with `xrandr` to modify display gamma/brightness values, while client commands communicate with the daemon for fast response times. Settings are automatically saved and restored between sessions.

This approach provides software-level brightness control that works independently of hardware backlight controls, making it especially useful for external monitors or systems where hardware brightness adjustment isn't available.
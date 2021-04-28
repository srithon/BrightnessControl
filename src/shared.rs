use lazy_static::lazy_static;

use bincode::{Options, DefaultOptions};

use serde::{Serialize, Deserialize};

use fields_converter_derive::*;

lazy_static! {
    pub static ref BINCODE_OPTIONS: DefaultOptions = {
        let options = bincode::DefaultOptions::default();
        options.with_fixint_encoding();
        options
    };
}

pub const SOCKET_PATH: &str = "/tmp/brightness_control_socket.sock";

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct BrightnessInput {
    pub brightness: Option<BrightnessChange>,
    pub override_fade: Option<bool>,
    pub override_monitor: Option<MonitorOverride>,
    pub terminate_fade: bool
}

impl BrightnessInput {
    pub fn is_active(&self) -> bool {
        self.brightness.is_some() || self.terminate_fade
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub enum BrightnessChange {
    Adjustment(i8),
    Set(u8)
}

// NOTE KEEP THESE ENUMS IN SYNC
/// Allows you to specify which monitors an action should affect
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum MonitorOverride {
    Specified { adapter_name: String },
    Active,
    Enabled,
    All
}

// NOTE KEEP THESE ENUMS IN SYNC
#[derive(Serialize, Deserialize, Debug, Clone, MoveFields)]
#[destinations("MonitorOverride")]
#[serde(tag = "type")]
// TODO NOTE this is an ugly solution
// bincode does not support explicit internal tagging, and will throw an error at runtime if you
// try to deserialize an enum that uses it
// we want the user to be able to specify the default MonitorOverride using internal tagging in the
// config file
// solution: duplicate the struct and use internal tagging for the copy, and use the From trait to
// convert it into the regular MonitorOverride
// we're using the fields-converter-derive crate to derive the From trait between these two types
// I wanted to phase out this duplicate definition by using the crate's Duplicate derive macro, but
// it wasn't as versatile as I needed
// see discussion here: https://github.com/serde-rs/serde/issues/1310
/// Duplicate of MonitorOverride which uses internal tagging for serialization
/// Internal tagging is necessary for TOML deserialization, but breaks Bincode deserialization
/// We need to use Bincode within the program, but the user has to be able to specify a default
/// MonitorOverride in their configuration file
/// This enum is used as an intermediate, and is immediately converted into a regular
/// MonitorOverride after deserializing it from the user config
pub enum MonitorOverrideTOMLCompatible {
    Specified { adapter_name: String },
    Active,
    Enabled,
    All
}

#[derive(Serialize, Deserialize, Debug)]
pub enum GetProperty {
    Brightness(Option<MonitorOverride>),
    IsFading(Option<MonitorOverride>),
    Mode,
    Displays,
    ActiveMonitor,
    Config
}

#[derive(Serialize, Deserialize, Debug)]
pub enum ActiveMonitorChange {
    /// The String field can be either a string representation of the adapter index OR the name of
    /// the adapter
    SetActive(String)
}

#[derive(Serialize, Deserialize, Debug)]
pub enum ProgramInput {
    Brightness(BrightnessInput),
    Get(GetProperty),
    ChangeActiveMonitor(ActiveMonitorChange),
    ConfigureDisplay,
    ToggleNightlight,
    ReloadConfiguration,
    Shutdown
}

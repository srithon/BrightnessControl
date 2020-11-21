use lazy_static::lazy_static;

use bincode::{Options, DefaultOptions};

use serde::{Serialize, Deserialize};

lazy_static! {
    pub static ref BINCODE_OPTIONS: DefaultOptions = {
        let options = bincode::DefaultOptions::default();
        options.with_fixint_encoding();
        options
    };
}

pub const SOCKET_PATH: &str = "/tmp/brightness_control_socket.sock";

#[derive(Serialize, Deserialize, Debug)]
pub struct BrightnessInput {
    pub brightness: Option<BrightnessChange>,
    pub override_fade: Option<bool>,
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

#[derive(Serialize, Deserialize, Debug)]
pub enum GetProperty {
    Brightness,
    IsFading,
    Mode,
    Displays,
    Config
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ProgramInput {
    pub brightness: BrightnessInput,
    pub get_property: Option<GetProperty>,
    pub configure_display: bool,
    pub toggle_nightlight: bool,
    pub reload_configuration: bool,
    pub shutdown: bool
}

impl ProgramInput {
    pub fn new(brightness: BrightnessInput, get_property: Option<GetProperty>, configure_display: bool, toggle_nightlight: bool, reload_configuration: bool, shutdown: bool) -> ProgramInput {
        ProgramInput {
            brightness,
            get_property,
            configure_display,
            toggle_nightlight,
            reload_configuration,
            shutdown,
        }
    }

    pub fn returns_feedback(&self) -> bool {
        // create a vector of all options that send back feedback to the client
        let feedback_returning_options = vec![
            self.reload_configuration
        ];

        // returns true if any of them are true
        feedback_returning_options.iter().any(|&b| b)
    }
}

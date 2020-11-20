use lazy_static::lazy_static;

use serde::{Serialize, Deserialize};

pub const CONFIG_TEMPLATE: &str = include_str!("../../../config_template.toml");

lazy_static! {
    static ref DEFAULT_CONFIG: DaemonOptions = {
        let parsed_toml: DaemonOptions = toml::from_slice(CONFIG_TEMPLATE.as_bytes()).unwrap();
        parsed_toml
    };
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct NightlightOptions {
    pub xrandr_gamma: String,
    pub redshift_temperature: u32
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FadeOptions {
    pub threshold: u8,
    // milliseconds
    pub total_duration: u32,
    // milliseconds
    pub step_duration: u32
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DaemonOptions {
    pub use_redshift: bool,
    pub auto_remove_displays: bool,
    pub fade_options: FadeOptions,
    pub nightlight_options: NightlightOptions
}

impl DaemonOptions {
    pub fn default() -> DaemonOptions {
        (*DEFAULT_CONFIG).clone()
    }
}

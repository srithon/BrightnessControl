use lazy_static::lazy_static;

use serde::{Deserialize, Serialize};

use crate::shared::*;

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
    pub redshift_temperature: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FadeOptions {
    pub threshold: u8,
    // milliseconds
    pub total_duration: u32,
    // milliseconds
    pub step_duration: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DaemonOptions {
    pub use_redshift: bool,
    pub auto_remove_displays: bool,
    pub fade_options: FadeOptions,
    pub nightlight_options: NightlightOptions,

    // discussion here: https://github.com/serde-rs/serde/issues/1310
    #[serde(deserialize_with = "parse_monitor_default_behavior")]
    pub monitor_default_behavior: MonitorOverride,
}

/// Parses a MonitorOverrideTOMLCompatible and converts it into a regular MonitorOverride
fn parse_monitor_default_behavior<'de, D>(input: D) -> Result<MonitorOverride, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // https://stackoverflow.com/a/46755370
    let monitor_override_toml: MonitorOverrideTOMLCompatible = Deserialize::deserialize(input)?;
    Ok(monitor_override_toml.into())
}

impl DaemonOptions {
    pub fn default() -> DaemonOptions {
        (*DEFAULT_CONFIG).clone()
    }
}

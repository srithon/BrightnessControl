use std::io::Result;

use tokio::sync::RwLock;

use tokio::process::Command;

use std::collections::BTreeSet;

use lazy_static::lazy_static;

use regex::Regex;

use fnv::FnvHashMap;

use crate::daemon::config::runtime::BrightnessState;
use crate::shared::*;

lazy_static! {
    static ref XRANDR_DISPLAY_INFORMATION_REGEX: Regex = {
        Regex::new(r"(?x) # ignore whitespace
        # [[:alpha:]] represents ascii letters
        ^([[:alpha:]]+-[[:digit:]]+) # 0 : the adapter name
        \ # space
        # 1 : 'disconnected' or 'connected ...'
        (
        disconnected
        |
        connected
        \ # space
        .*? # optional other words
        ([[:digit:]]+) # 2 : width
        x
        ([[:digit:]]+) # 3 : height
        \+
        ([[:digit:]]+) # 4 : x_offset
        \+
        ([[:digit:]]+) # 5 : y_offset
        )
        ").unwrap()
    };
}

pub struct MonitorMetadata {
    pub width: u32,
    pub height: u32,
    pub x_offset: u32,
    pub y_offset: u32
}

// encapsulates information from xrandr --current
pub struct Monitor {
    adapter_name: String,
    monitor_metadata: Option<MonitorMetadata>
}

impl Monitor {
    // returns (Monitor, connected)
    pub fn new(xrandr_line: &str) -> Option<(Monitor, bool)> {
        // eDP-1 connected primary 1920x1080+0+0 (normal left inverted right x axis y axis) 344mm x 193mm
        // HDMI-1 connected 1280x1024+1920+28 (normal left inverted right x axis y axis) 338mm x 270mm
        // <adapter> connected [primary] <width>x<height>+<x offset>+<y offset> (<flags>) <something>mm x <something else>mm
        let captures = (*XRANDR_DISPLAY_INFORMATION_REGEX).captures(xrandr_line);

        if let Some(captures) = captures {
            // 0 points to the entire match, so skip
            let adapter_name = captures.get(1).unwrap().as_str().to_owned();

            let parse_int = | num: regex::Match | {
                num.as_str().parse::<u32>()
            };

            (|| {
                let connected = {
                    if let Some(capture) = captures.get(2) {
                        !"disconnected".eq(capture.as_str())
                    }
                    else {
                        false
                    }
                };

                let monitor_metadata = {
                    if connected {
                        let width = parse_int(captures.get(3).unwrap())?;
                        let height = parse_int(captures.get(4).unwrap())?;
                        let x_offset = parse_int(captures.get(5).unwrap())?;
                        let y_offset = parse_int(captures.get(6).unwrap())?;

                        Some(MonitorMetadata {
                            width,
                            height,
                            x_offset,
                            y_offset
                        })
                    }
                    else {
                        None
                    }
                };

                let monitor = Monitor {
                    adapter_name,
                    monitor_metadata
                };

                std::result::Result::<Option<(Monitor, bool)>, std::num::ParseIntError>::Ok(
                    Some( (monitor, connected) )
                )
            })().unwrap_or(None)
        }
        else {
            None
        }
    }

    pub fn name(&self) -> &String {
        &self.adapter_name
    }

    fn update_metadata(&mut self, new_metadata: Option<MonitorMetadata>) {
        self.monitor_metadata = new_metadata;
    }
}

pub struct MonitorState {
    pub monitor_data: Monitor,
    pub brightness_state: BrightnessState
}

impl MonitorState {
    pub fn get_monitor_name(&self) -> &String {
        self.monitor_data.name()
    }

    pub fn get_brightness_state(&self) -> &BrightnessState {
        &self.brightness_state
    }
}

/// contains all the BrightnessState information with mappings to their respective monitors
/// also keeps track of which monitors are enabled and which are disabled
pub struct CollectiveMonitorStateInternal {
    /// list of ALL MonitorState's, even those that are inactive/disabled/disconnected
    available_adapter_list: Vec<MonitorState>,
    /// list of monitor indices within available_adapter_list which are connected/usable
    enabled_monitors: BTreeSet<usize>,
    /// index of the "active" monitor
    active_monitor: usize,
    /// map between the name of each adapter and its index within the list
    monitor_names: FnvHashMap<String, usize>
}

impl CollectiveMonitorStateInternal {
    pub async fn new(active_monitor: usize, brightness_states: FnvHashMap<String, f64>) -> CollectiveMonitorStateInternal {
        let initial_capacity = brightness_states.len() + 3;

        let adapters = Vec::with_capacity(initial_capacity);
        let enabled_monitors = BTreeSet::new();
        let mut monitor_names = FnvHashMap::default();
        monitor_names.reserve(initial_capacity);

        let mut monitor_states = CollectiveMonitorStateInternal {
            available_adapter_list: adapters,
            enabled_monitors,
            active_monitor,
            monitor_names
        };

        // populates monitor_states fields
        if let Err(e) = monitor_states.refresh_displays().await {
            eprintln!("Error refreshing displays: {}", e);
        }

        for (adapter_name, brightness) in brightness_states {
            if let Some(monitor_state) = monitor_states.get_monitor_state_by_name(&adapter_name) {
                monitor_state.brightness_state.brightness.set_value(brightness).await;
            }
        }

        monitor_states
    }

    pub fn get_monitor_state_by_name(&self, name: &str) -> Option<&MonitorState> {
        let index = self.get_monitor_index_by_name(name);

        match index {
            Some(&index) => {
                self.available_adapter_list.get(index)
            },
            _ => None
        }
    }

    pub fn get_monitor_state_by_index(&self, index: usize) -> Option<&MonitorState> {
        self.available_adapter_list.get(index)
    }

    pub fn iter_all_monitor_indices(&self) -> impl Iterator<Item=usize> + '_ {
        (0..self.available_adapter_list.len()).into_iter()
    }

    pub fn iter_enabled_monitor_indices(&self) -> impl Iterator<Item=usize> + '_ {
        self.enabled_monitors.iter().map(|&x| x)
    }

    pub fn iter_enabled_monitor_states(&self) -> impl Iterator<Item=&MonitorState> + '_ {
        // TODO filter map?
        // ensure that the unwrap is guaranteed
        self.enabled_monitors.iter().map(move |&x| self.available_adapter_list.get(x).unwrap())
    }

    pub fn iter_all_monitor_states(&self) -> impl Iterator<Item=&MonitorState> + '_ {
        self.available_adapter_list.iter()
    }

    // Monitor, is_connected
    pub fn state_overwrite(&mut self, displays_list: Vec<(Monitor, bool)>) {
        // clear the connected_displays set
        // for each display
        //  overwrite monitor metadata IF already present
        //  if not present, append to the end with with default BrightnessState
        //  add the returned indice to connected_displays
        self.clear_enabled_monitors();

        // this might not be necessary if the order doesnt change, but to be safe
        self.clear_monitor_names();

        for display in displays_list {
            let index = self.overwrite_monitor_metadata(display.0);

            // is_connected is true
            if display.1 {
                self.add_enabled_monitor(index);
            }
        }
    }

    pub async fn refresh_displays(&mut self) -> Result<()> {
        let available_displays = get_available_displays().await;

        match available_displays {
            Ok(displays) => {
                self.state_overwrite(displays);
                Ok( () )
            },
            Err(e) => Err(e)
        }
    }

    pub fn get_monitor_index_by_name(&self, name: &str) -> Option<&usize> {
        self.monitor_names.get(&name.to_ascii_lowercase())
    }

    pub fn set_active_monitor_by_name(&mut self, new_active_monitor_name: &str) -> std::result::Result<(), &str> {
        match self.get_monitor_index_by_name(new_active_monitor_name) {
            // TODO use better errors here
            Some(&index) => {
                if self.is_monitor_index_enabled(index) {
                    self.active_monitor = index;
                    Ok(())
                }
                else {
                    Err("Requested monitor is disabled!")
                }
            },
            None => Err("Requested monitor does not exist!")
        }
    }

    // NOTE if we dont specify that the str in the return type is static, we will get lifetime
    // issues because the borrow checker will think that the str lifetime is tied to that of 'self'
    pub fn set_active_monitor_by_index(&mut self, new_active_monitor_index: usize) -> std::result::Result<(), &'static str> {
        // if the index is greater than or equal to the length of the adapter list, it is out of bounds
        if self.available_adapter_list.len() <= new_active_monitor_index {
            return Err("Specified index does not exist!");
        }
        else if !self.is_monitor_index_enabled(new_active_monitor_index) {
            return Err("Specified monitor is not enabled!");
        }
        else {
            self.active_monitor = new_active_monitor_index;
            Ok(())
        }
    }

    pub fn get_active_monitor_index(&self) -> &usize {
        &self.active_monitor
    }

    // returns Some(index) if the monitor was found, otherwise None
    fn overwrite_monitor_metadata(&mut self, monitor: Monitor) -> usize {
        if let Some(&index) = self.get_monitor_index_by_name(&monitor.name()) {
            self.available_adapter_list.get_mut(index).unwrap().monitor_data.update_metadata(monitor.monitor_metadata);
            index
        }
        else {
            let adapter_name = monitor.adapter_name.to_ascii_lowercase();

            let monitor_state = MonitorState {
                brightness_state: BrightnessState::new(100.0),
                monitor_data: monitor
            };

            self.available_adapter_list.push(monitor_state);

            let index = self.available_adapter_list.len() - 1;
            self.monitor_names.insert(adapter_name, index);

            index
        }
    }

    fn clear_monitor_names(&mut self) {
        self.monitor_names.clear()
    }

    fn clear_enabled_monitors(&mut self) {
        self.enabled_monitors.clear()
    }

    fn add_enabled_monitor(&mut self, index: usize) {
        self.enabled_monitors.insert(index);
    }

    pub fn is_monitor_name_enabled(&self, monitor_name: &str) -> bool {
        if let Some(index) = self.get_monitor_index_by_name(monitor_name) {
            self.enabled_monitors.contains(&index)
        }
        else {
            false
        }
    }

    pub fn is_monitor_index_enabled(&self, monitor_index: usize) -> bool {
        self.enabled_monitors.contains(&monitor_index)
    }

    pub fn remove_enabled_monitor_by_name(&mut self, monitor_name: &str) {
        if let Some(&index) = self.get_monitor_index_by_name(monitor_name) {
            self.enabled_monitors.remove(&index);
        }
    }

    pub fn remove_enabled_monitor_by_index(&mut self, monitor_index: &usize) {
        // TODO assert that the index is within the set?
        // would simply assert the return value of this call
        self.enabled_monitors.remove(monitor_index);
    }

    // TODO clean this up
    // this should really be an internal function
    // reasoning:
    // cant return the iterator itself with static dispatch
    // because in order to use static dispatch (impl Iterator), there cannot be a branch in the
    // return
    // to work around this, we do the mapping within the function itself and then return the
    // normalized result: a vector
    // generalized map to for_each
    pub fn for_each_monitor_override_iterator(&self, monitors: &MonitorOverride, closure: &mut impl FnMut(usize)) {
        match monitors {
            MonitorOverride::All => {
                let iterator = self.iter_all_monitor_indices();
                iterator.for_each(closure)
            },
            MonitorOverride::Enabled => {
                let iterator = self.iter_enabled_monitor_indices();
                iterator.for_each(closure)
            },
            x => {
                let index = match x {
                    MonitorOverride::Specified { ref adapter_name } => {
                        // TODO remove this copy
                        // either do it in-place (mutably) or sanitize the input beforehand by
                        // making it lowercase
                        self.get_monitor_index_by_name(&adapter_name)
                    },
                    MonitorOverride::Active => {
                        Some(self.get_active_monitor_index())
                    },
                    _ => unreachable!("Already took care of All case!")
                };

                match index {
                    Some(&index) => {
                        closure(index)
                    },
                    None => ()
                };
            }
        }
    }

    pub fn get_monitor_override_indices(&self, monitors: &MonitorOverride) -> Vec<usize> {
        match monitors {
            MonitorOverride::All => {
                let iterator = self.iter_all_monitor_indices();
                iterator.collect()
            },
            MonitorOverride::Enabled => {
                let iterator = self.iter_enabled_monitor_indices();
                iterator.collect()
            },
            x => {
                let index = match x {
                    MonitorOverride::Specified { ref adapter_name } => {
                        // TODO remove this copy
                        // either do it in-place (mutably) or sanitize the input beforehand by
                        // making it lowercase
                        self.get_monitor_index_by_name(&adapter_name)
                    },
                    MonitorOverride::Active => {
                        Some(self.get_active_monitor_index())
                    },
                    _ => unreachable!("Already took care of All case!")
                };

                match index {
                    Some(&index) => {
                        vec![index]
                    },
                    None => vec![]
                }
            }
        }
    }
}

/// RwLock wrapper around CollectiveMonitorStateInternal
/// Provides thread-safety to single-threaded object
pub struct CollectiveMonitorState {
    pub monitor_states: RwLock<CollectiveMonitorStateInternal>
}

#[macro_use]
/// Private module to hide macro from importing modules
mod private_macros {
    /// Given an optional MonitorOverride and a function that takes a MonitorState and returns
    /// something convertible into a String, returns a concatenation of all the resulting Strings
    /// joined together with newlines
    /// If monitor_override is None, default to using all monitors (disabled and enabled)
    macro_rules! collect_formatted_displays_string {
        ($self:expr, $monitor_override:expr, $closure:expr, $separator:expr) => {
            {
                // exclusive read
                let guard = $self.monitor_states.read().await;

                if $monitor_override.is_none() {
                    guard
                        .iter_all_monitor_states()
                        .map($closure)
                        .collect::<Vec<_>>()
                        .join($separator)
                }
                else {
                    guard
                        .get_monitor_override_indices($monitor_override.unwrap())
                        .into_iter()
                        .map(|monitor_index| guard.get_monitor_state_by_index(monitor_index).unwrap())
                        .map($closure)
                        .collect::<Vec<_>>()
                        .join($separator)
                }
            }
        }
    }
}

impl CollectiveMonitorState {
    pub async fn new(active_monitor: usize, brightness_states: FnvHashMap<String, f64>) -> CollectiveMonitorState {
        let internal = CollectiveMonitorStateInternal::new(active_monitor, brightness_states).await;

        CollectiveMonitorState {
            monitor_states: RwLock::new(internal)
        }
    }

    /// Returns space-separated list of display names matched by monitor_override
    pub async fn get_formatted_display_names(&self, monitor_override: Option<&MonitorOverride>) -> String {
        collect_formatted_displays_string!(self, monitor_override, |monitor_state| monitor_state.monitor_data.adapter_name.as_ref(), " ")
    }

    /// Returns newline-separated list of "<display name>: <brightness level>" for displays matched
    /// by monitor_override
    pub async fn get_formatted_display_states(&self, monitor_override: Option<&MonitorOverride>) -> String {
        collect_formatted_displays_string!(self, monitor_override, |monitor_state|
            format!("{}: {}",
                monitor_state.monitor_data.adapter_name,
                monitor_state.brightness_state.brightness.get()
            ),
            "\n"
        )
    }

    /// Returns newline-separated list of "<display name>: <result of closure(&MonitorState)>" for
    /// displays matched by monitor_override
    pub async fn get_formatted_display_states_with_format<T: std::fmt::Display>(&self, monitor_override: Option<&MonitorOverride>, closure: impl Fn(&MonitorState) -> T) -> String
    {
        collect_formatted_displays_string!(self, monitor_override, |monitor_state|
            format!("{}: {}",
                monitor_state.monitor_data.adapter_name,
                closure(monitor_state)
            ),
            "\n"
        )
    }

    pub async fn read(&self) -> tokio::sync::RwLockReadGuard<'_, CollectiveMonitorStateInternal> {
        self.monitor_states.read().await
    }

    pub async fn write(&self) -> tokio::sync::RwLockWriteGuard<'_, CollectiveMonitorStateInternal> {
        self.monitor_states.write().await
    }
}

/// Parses `xrandr --current` output and returns a list of (Monitor, is_monitor_connected: bool)
pub async fn get_available_displays() -> Result<Vec<(Monitor, bool)>> {
    let mut xrandr_current = Command::new("xrandr");
    xrandr_current.arg("--current");
    let command_output = xrandr_current.output().await?;
    // the '&' operator dereferences ascii_code so that it can be compared with a regular u8
    // its original type is &u8
    let output_lines = command_output.stdout.split(| &ascii_code | ascii_code == b'\n');

    let displays: Vec<(Monitor, bool)> = output_lines.filter_map(|line| {
        // if valid UTF-8, pass to Monitor
        if let Ok(line) = std::str::from_utf8(line) {
            Monitor::new(line)
        }
        else {
            None
        }
    }).collect();

    Ok(displays)
}

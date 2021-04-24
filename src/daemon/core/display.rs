use std::io::Result;

use tokio::process::Command;

use lazy_static::lazy_static;

use regex::Regex;


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

pub async fn get_current_connected_displays() -> Result<Vec<Monitor>> {
    let mut xrandr_current = Command::new("xrandr");
    xrandr_current.arg("--current");
    let command_output = xrandr_current.output().await?;
    // the '&' operator dereferences ascii_code so that it can be compared with a regular u8
    // its original type is &u8
    let output_lines = command_output.stdout.split(| &ascii_code | ascii_code == b'\n');

    let connected_displays: Vec<Monitor> = output_lines.filter_map(|line| {
        // if valid UTF-8, pass to Monitor
        if let Ok(line) = std::str::from_utf8(line) {
            Monitor::new(line)
        }
        else {
            None
        }
    }).collect();

    Ok(connected_displays)
}

pub async fn configure_displays() -> Result<Vec<Monitor>> {
    let connected_displays = get_current_connected_displays().await?;

    Ok(connected_displays)
}

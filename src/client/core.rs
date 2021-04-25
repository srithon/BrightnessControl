use bincode::Options as BincodeOptions;
use clap::ArgMatches;

use std::io::{Write, Result};

use std::os::unix::net::UnixStream;

use std::io::{BufRead, BufReader};

use crate::shared::*;

fn check_brightness(matches: &ArgMatches, override_monitor: Option<MonitorOverride>) -> BrightnessInput {
    if let Some(brightness_matches) = matches.subcommand_matches("brightness") {
        let brightness_change = if let Some(new_brightness) = brightness_matches.value_of("set") {
            // unwrap because caller should be doing input validation
            Some(BrightnessChange::Set(new_brightness.parse::<u8>().unwrap()))
        }
        else {
            (|| {
                let (num_string, multiplier) = {
                    if let Some(brightness_shift) = brightness_matches.value_of("increment") {
                        (brightness_shift, 1)
                    }
                    else if let Some(brightness_shift) = brightness_matches.value_of("decrement") {
                        (brightness_shift, -1)
                    }
                    else {
                        return None;
                    }
                };

                let num = num_string.parse::<i8>().unwrap();

                Some(BrightnessChange::Adjustment(num * multiplier))
            })()
        };

        let override_fade = {
            if matches.is_present("force_fade") {
                Some(true)
            }
            else if matches.is_present("force_no_fade") {
                Some(false)
            }
            else {
                None
            }
        };

        let terminate_fade = matches.is_present("terminate_fade");

        BrightnessInput {
            brightness: brightness_change,
            override_fade,
            override_monitor,
            terminate_fade
        }
    } else {
        BrightnessInput::default()
    }
}

fn check_get_property(matches: &ArgMatches, monitor_override: &Option<MonitorOverride>) -> Option<GetProperty> {
    if let Some(get_subcommand) = matches.subcommand_matches("get") {
        let present = |name| {
            get_subcommand.is_present(name)
        };

        if get_subcommand.is_present("get_request") {
            let res = if present("brightness") {
                GetProperty::Brightness(monitor_override.clone())
            }
            else if present("mode") {
                GetProperty::Mode
            }
            else if present("displays") {
                GetProperty::Displays
            }
            else if present("fading") {
                GetProperty::IsFading(monitor_override.clone())
            }
            else if present("config") {
                GetProperty::Config
            }
            else {
                unreachable!("Invalid get argument")
            };

            Some(res)
        }
        else {
            None
        }
    }
    else {
        None
    }
}

fn check_monitor_override(matches: &clap::ArgMatches) -> Option<MonitorOverride> {
    if matches.is_present("monitor_override") {
        let monitor_override = if let Some(monitor) = matches.value_of("monitor") {
            MonitorOverride::Specified { adapter_name: monitor.to_string() }
        }
        else if matches.is_present("active") {
            MonitorOverride::Active
        }
        else {
            MonitorOverride::All
        };

        Some(monitor_override)
    }
    else {
        None
    }
}

pub fn handle_input(matches: &clap::ArgMatches) -> Result<()> {
    let monitor_override = check_monitor_override(&matches);
    let get_property = check_get_property(&matches, &monitor_override);
    let brightness_subcommand = check_brightness(&matches, monitor_override);

    let program_input = ProgramInput::new(
        brightness_subcommand,
        get_property,
        matches.is_present("configure_display"),
        matches.is_present("toggle_nightlight"),
        matches.is_present("reload_configuration"),
        false,
    );

    // SEND INPUT TO DAEMON
    let mut socket = match UnixStream::connect(SOCKET_PATH) {
        Ok(sock) => sock,
        Err(_) => {
            eprintln!("Couldn't connect to socket.");
            eprintln!("Start the daemon with the \"--daemon\" option, and then try again");
            eprintln!("If the daemon is already running, terminate it with \"killall brightness_control\" and then relaunch it");
            std::process::exit(1);
        }
    };

    let bincode_options = &BINCODE_OPTIONS;
    let binary_encoded_input = bincode_options.serialize(&program_input).unwrap();

    socket.write_all(&binary_encoded_input)?;

    if !matches.is_present("quiet") {
        // TODO figure out if a read timeout is necessary
        let buffered_reader = BufReader::with_capacity(512, &mut socket);
        for line in buffered_reader.lines() {
            match line {
                Ok(line) => println!("{}", line),
                Err(e) => eprintln!("Failed to read line: {}", e)
            };
        }
    }

    socket.shutdown(std::net::Shutdown::Both)?;

    // IF DAEMON IS NOT RUNNING, THROW ERROR

    Ok(())
}

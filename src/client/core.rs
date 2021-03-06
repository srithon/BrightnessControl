use bincode::Options as BincodeOptions;
use clap::ArgMatches;

use std::io::{Write, Result};

use std::os::unix::net::UnixStream;

use std::io::{BufRead, BufReader};

use crate::shared::*;

fn check_brightness(matches: &ArgMatches) -> Result<BrightnessInput> {
    let brightness = {
        if let Some(new_brightness) = matches.value_of("set") {
            // unwrap because caller should be doing input validation
            Some(BrightnessChange::Set(new_brightness.parse::<u8>().unwrap()))
        }
        else {
            (|| {
                let (num_string, multiplier) = {
                    if let Some(brightness_shift) = matches.value_of("increment") {
                        (brightness_shift, 1)
                    }
                    else if let Some(brightness_shift) = matches.value_of("decrement") {
                        (brightness_shift, -1)
                    }
                    else {
                        return None;
                    }
                };

                let num = num_string.parse::<i8>().unwrap();

                Some(BrightnessChange::Adjustment(num * multiplier))
            })()
        }
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

    Ok(
        BrightnessInput {
            brightness,
            override_fade,
            terminate_fade
        }
    )
}

fn check_get_property(matches: &ArgMatches) -> Option<GetProperty> {
    if let Some(get_argument) = matches.value_of("get") {
        if let Some(mut first_char) = get_argument.chars().next() {
            first_char.make_ascii_lowercase();
            return match first_char {
                'b' => Some(GetProperty::Brightness),
                'm' => Some(GetProperty::Mode),
                'd' => Some(GetProperty::Displays),
                'c' => Some(GetProperty::Config),
                'i' => Some(GetProperty::IsFading),
                _ => None
            }
        }
    }

    None
}

pub fn handle_input(matches: &clap::ArgMatches) -> Result<()> {
    let brightness_subcommand = check_brightness(&matches)?;
    let get_property = check_get_property(&matches);

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

use bincode::Options as BincodeOptions;
use getopts::Matches;

use std::io::{Error, ErrorKind, Write, Result};

use std::os::unix::net::UnixStream;

use crate::daemon::*;

fn check_brightness(matches: &Matches) -> Result<Option<BrightnessChange>> {
    let increment = matches.opt_str("increment");
    let decrement = matches.opt_str("decrement");
    let set = matches.opt_str("set");

    let result: Result<Option<BrightnessChange>> = (|| {
        let invalid_input = | message | {
            Err(Error::new(ErrorKind::InvalidInput, message))
        };

        let duplicate_arguments = || {
            invalid_input("Only one of {increment, decrement, set} can be used at the same time!")
        };

        if increment.is_some() || decrement.is_some() {
            if set.is_some() || increment.is_some() && decrement.is_some() {
                return duplicate_arguments();
            }

            let (num, multiplier) = {
                if increment.is_some() {
                    (increment.unwrap(), 1)
                }
                else {
                    (decrement.unwrap(), -1)
                }
            };

            // "num" is a valid number
            if let Ok(num) = num.parse::<i8>() {
                if num <= 100 && num >= -100 {
                    Ok(Some(BrightnessChange::Adjustment(num * multiplier)))
                }
                else {
                    invalid_input("Brightness adjustment must be between -100 and 100!")
                }
            }
            else {
                invalid_input("Invalid number passed as decrement/increment")
            }
        }
        else if let Some(set) = set {
            // ^ use if let here because there is only 1 variable to worry about
            if decrement.is_some() {
                return duplicate_arguments();
            }

            if let Ok(new_brightness) = set.parse::<u8>() {
                Ok(Some(BrightnessChange::Set(new_brightness)))
            }
            else {
                invalid_input("Invalid brightness passed to set")
            }
        }
        else {
            Ok(None)
        }
    })();

    result
}

pub fn handle_input(matches: Matches) -> Result<()> {
    let brightness = check_brightness(&matches)?;

    let program_input = ProgramInput::new(
        brightness,
        matches.opt_present("configure-display"),
        matches.opt_present("toggle-nightlight"),
        false
    );

    // SEND INPUT TO DAEMON
    let mut socket = match UnixStream::connect(SOCKET_PATH) {
        Ok(sock) => sock,
        Err(e) => {
            eprintln!("Couldn't connect to socket.");
            eprintln!("Start the daemon with the \"--daemon\" option, and then try again");
            eprintln!("If the daemon is already running, terminate it with \"killall brightness_control\" and then relaunch it");
            std::process::exit(1);
        }
    };

    let bincode_options = get_bincode_options();
    let binary_encoded_input = bincode_options.serialize(&program_input).unwrap();

    socket.write_all(&binary_encoded_input)?;

    socket.shutdown(std::net::Shutdown::Both)?;

    // IF DAEMON IS NOT RUNNING, THROW ERROR

    Ok(())
}

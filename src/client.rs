use getopts::{Options, Matches};
use directories::ProjectDirs;

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Error, ErrorKind, Seek, SeekFrom, Write, Read, Result};
use std::fmt::Display;
use std::process::Command;

use std::cmp;

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
        matches.opt_present("toggle-nightlight")
    );

    // SEND INPUT TO DAEMON

    // IF DAEMON IS NOT RUNNING, THROW ERROR

    Ok(())
}

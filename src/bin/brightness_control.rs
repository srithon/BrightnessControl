use getopts::Options;
use directories::ProjectDirs;

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Error, ErrorKind, Seek, SeekFrom, Write, Read, Result};
use std::fmt::Display;
use std::process::Command;

use std::cmp;

use crate::daemon;
use crate::client;

fn get_cli_interface() -> Options {
    let mut options = Options::new();
    options
        .parsing_style(getopts::ParsingStyle::FloatingFrees)
        .optflag("h", "help", "Prints the help menu")
        .optflag("c", "configure-display", "Uses the current display configuration for future calls to BrightnessControl")
        .optflag("t", "toggle-nightlight", "Toggles the nightlight mode on/off")
        .optopt("s", "set", "Sets the current brightness to some percentage [0..100]", "PERCENTAGE")
        .optopt("i", "increment", "Increments the current brightness by some (integer) percentage between -100 and +100", "PERCENTAGE")
        .optopt("d", "decrement", "Decrements the current brightness by some (integer) percentage between -100 and +100", "PERCENTAGE");
    options
}

fn print_help(program_invocation_name: &str, cli: &Options) {
    let brief = format!("Usage: {} [options]", program_invocation_name);
    print!("{}", cli.usage(&brief));
}

fn main() -> Result<()> {
    let cli = get_cli_interface();
    let args = std::env::args().collect::<Vec<String>>();

    // this lets you see exactly how the user invoked the program
    // this is then used in the help screen
    let program_invocation_name = args[0].clone();

    let matches = match cli.parse(&args[1..]) {
        Ok(matches) => matches,
        Err(error) => {
            let error_string = error.to_string();
            eprintln!("Syntax error: {}\n", error_string);
            print_help(&program_invocation_name, &cli);

            // if an Err is returned instead, the Termination trait automatically serializes the
            // Err and prints it out
            // we do not want anything to be printed out, so instead of returning an Err, we
            // manually terminate the program with exit code 1
            std::process::exit(1);
        }
    };

    if matches.opt_present("help") {
        print_help(&program_invocation_name, &cli);
    }
    else if matches.opt_present("daemon") {
        // CHECK IF DAEMON IS RUNNING

        // START DAEMON
        daemon::daemon();
    }
    else {
        // RUN CLIENT CODE
        client::handle_input(matches);
    }

    Ok(())
}

extern crate brightness_control;

use getopts::Options;

use std::io::Result;

use brightness_control::{daemon, client};

fn get_cli_interface() -> Options {
    let mut options = Options::new();
    options
        .parsing_style(getopts::ParsingStyle::FloatingFrees)
        .optflag("h", "help", "Prints the help menu")
        .optflag("v", "version", "Prints the current version of BrightnessControl")
        // b for BrightnessControl
        // already used 'd' for decrement and I didnt want to replace increment/decrement with 'adjustment'
        .optflag("b", "daemon", "Starts the BrightnessControl daemon")
        .optflag("c", "configure-display", "Uses the current display configuration for future calls to BrightnessControl")
        .optflag("t", "toggle-nightlight", "Toggles the nightlight mode on/off")
        .optflag("r", "reload-configuration", "Tells the daemon to re-read the configuration file and apply new changes")
        .optflag("", "print-default-config", "Prints out the default configuration template")
        .optflag("q", "quiet", "Do not wait for the Daemon's output before terminating")
        .optopt("s", "set", "Sets the current brightness to some percentage [0..100]", "PERCENTAGE")
        .optopt("i", "increment", "Increments the current brightness by some (integer) percentage between -100 and +100", "PERCENTAGE")
        .optopt("d", "decrement", "Decrements the current brightness by some (integer) percentage between -100 and +100", "PERCENTAGE");
    options
}

fn print_help(program_invocation_name: &str, cli: &Options) {
    let brief = format!("Usage: {} [options]\nBrightnessControl {}", program_invocation_name, env!("CARGO_PKG_VERSION"));
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
    else if matches.opt_present("version") {
        println!("BrightnessControl {}", env!("CARGO_PKG_VERSION"));
    }
    else if matches.opt_present("print-default-config") {
        println!("{}", daemon::CONFIG_TEMPLATE);
    }
    else if matches.opt_present("daemon") {
        // START DAEMON
        daemon::daemon()?;
    }
    else {
        // RUN CLIENT CODE
        client::handle_input(matches)?;
    }

    Ok(())
}

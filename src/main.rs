extern crate brightness_control;

use clap::clap_app;
use clap::crate_version;
use clap::AppSettings;

use std::io::Result;

use brightness_control::{daemon, client};

fn get_cli_interface() -> clap::App<'static, 'static> {
    let percentage_validator = |arg: String| {
        if let Ok(num) = arg.parse::<i16>() {
            if num <= 100 && num >= -100 {
                Ok(())
            }
            else {
                Err("Percentage out of bounds!".to_owned())
            }
        }
        else {
            Err("Invalid percentage!".to_owned())
        }
    };

    clap_app!(BrightnessControl =>
        (@setting VersionlessSubcommands)
        (version: crate_version!())
        (author: "Sridaran Thoniyil")
        (about: "BrightnessControl is an XRandr interface which allows users to make relative brightness adjustments easily.")
        (@subcommand brightness =>
            (about: "Holds commands involving brightness modification")
            (@setting AllowNegativeNumbers)
            (@group action =>
                (@arg increment: -i --increment +takes_value value_name[PERCENTAGE] {percentage_validator} "Increases the current brightness by %")
                (@arg decrement: -d --decrement +takes_value value_name[PERCENTAGE] {percentage_validator} "Decrements the current brightness by %")
                (@arg set: -s --set +takes_value value_name[PERCENTAGE] {percentage_validator} "Sets the current brightness to %")
            )
            (@arg force_fade: -f --fade "Overrides the auto-fade functionality and fades regardless of the current configuration")
            (@arg force_no_fade: -n --("no-fade") "Overrides the auto-fade functionality and does not fade regardless of the current configuration")
        )
        (@subcommand nightlight =>
            (about: "Holds commands relating to the nightlight")
            (@group action =>
                (@arg toggle_nightlight: -t --toggle "Toggles the nightlight")
            )
        )
        (@subcommand config =>
            (about: "Holds commands involving daemon configuration")
            (@group action =>
                (@arg reload_configuration: -r --reload "Tells the daemon to read the current configuration from the configuration file")
                (@arg print_default: -p --("print-default") "Prints out the default daemon configuration")
            )
        )
        (@arg get: -g --get +takes_value value_name[property] possible_value[brightness configuration displays mode] "Holds commands that return feedback from the daemon")
        (@arg configure_display: -c --("configure-display") "Uses the current display configuration for future calls to BrightnessControl")
        (@arg quiet: -q --quiet +global "Do not wait for the Daemon's output before terminating")
        (@subcommand daemon =>
            (about: "Holds commands relating to the daemon lifecycle")
            (@group action =>
                (@arg start: -s --start "Attempts to start the daemon. The process will not be tied to the process that runs it")
            )
        )
    ).global_setting(AppSettings::ArgRequiredElseHelp)
}

fn main() -> Result<()> {
    let cli = get_cli_interface();

    let matches = cli.get_matches();

    let parsed_matches = (|| -> Result<&clap::ArgMatches> {
        match matches.subcommand() {
            ("daemon", Some(sub_app)) => {
                if sub_app.is_present("start") {
                    daemon::daemon()?;
                }
            },
            ("config", Some(sub_app)) => {
                if sub_app.is_present("print-default") {
                    println!("{}", daemon::CONFIG_TEMPLATE);
                }
                else {
                    return Ok( sub_app );
                }
            },
            // match all
            (_, subcommand) => {
                // if there is a subcommand, return that
                // otherwise, just return the base matches object
                // -- this allows parsing of base-level arguments
                return if let Some(subcommand) = subcommand {
                    Ok( subcommand )
                }
                else {
                    Ok( &matches )
                }
            }
        };

        std::process::exit(0);
    })()?;

    client::handle_input(parsed_matches)?;

    Ok(())
}

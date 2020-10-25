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

    let property_validator = |arg: String| {
        const ERROR_MESSAGE: &str = "Valid options are [b]rightness, [c]onfiguration, [d]isplays, and [m]ode";

        if arg.len() == 1 {
            const CHARS: &[char] = &['b', 'c', 'd', 'm'];
            let arg_char = arg.chars().nth(0).unwrap();
            let valid = CHARS.iter().any(|c| arg_char.eq_ignore_ascii_case(c));

            return if !valid {
                Err(ERROR_MESSAGE.to_owned())
            }
            else {
                Ok(())
            }
        }
        else {
            const OPTIONS: &[&str] = &["brightness", "configuration", "displays", "mode"];
            let valid = OPTIONS.iter().any(|prop| arg.eq_ignore_ascii_case(prop));

            return if !valid {
                Err(ERROR_MESSAGE.to_owned())
            }
            else {
                Ok(())
            }
        }
    };

    clap_app!(BrightnessControl =>
        (@setting VersionlessSubcommands)
        (@setting ArgsNegateSubcommands)
        (version: crate_version!())
        (author: "Sridaran Thoniyil")
        (about: "BrightnessControl is an XRandr interface which allows users to make relative brightness adjustments easily.")
        (@subcommand brightness =>
            (about: "Holds commands involving brightness modification")
            (visible_alias: "b")
            (@setting AllowNegativeNumbers)
            (@group action =>
                (@arg increment: -i --increment +takes_value value_name[PERCENTAGE] {percentage_validator} "Increases the current brightness by %")
                (@arg decrement: -d --decrement +takes_value value_name[PERCENTAGE] {percentage_validator} "Decrements the current brightness by %")
                (@arg set: -s --set +takes_value value_name[PERCENTAGE] {percentage_validator} "Sets the current brightness to %")
            )
            (@arg quiet: -q --quiet "Do not wait for the Daemon's output before terminating")
            (@arg force_fade: -f --fade requires[action] "Overrides the auto-fade functionality and fades regardless of the current configuration")
            (@arg force_no_fade: -n --("no-fade") requires[action] "Overrides the auto-fade functionality and does not fade regardless of the current configuration")
            (@arg terminate_fade: -t --("terminate-fade") required_unless[action] "Terminates the current fade if one is currently running; this can be combined with one")
        )
        (@subcommand nightlight =>
            (about: "Holds commands relating to the nightlight")
            (visible_alias: "n")
            (@group action =>
                (@arg toggle_nightlight: -t --toggle "Toggles the nightlight")
            )
            (@arg quiet: -q --quiet requires[action] "Do not wait for the Daemon's output before terminating")
        )
        (@subcommand config =>
            (about: "Holds commands involving daemon configuration")
            (visible_alias: "c")
            (@group action =>
                (@arg reload_configuration: -r --reload "Tells the daemon to read the current configuration from the configuration file")
                (@arg print_default: -p --("print-default") "Prints out the default daemon configuration")
            )
        )
        (@arg get: -g --get +takes_value value_name[property] {property_validator} "Gets the current value of the specified property: 'b[rightness]', 'm[ode]', 'd[isplays]', or 'c[onfig]'")
        (@arg configure_display: -c --("configure-display") conflicts_with[get] "Uses the current display configuration for future calls to BrightnessControl")
        (@subcommand daemon =>
            (about: "Holds commands relating to the daemon lifecycle")
            (visible_alias: "d")
            (@group action =>
                (@arg start: -s --start "Attempts to start the daemon. The process will not be tied to the process that runs it")
            )
            (@arg no_fork: -n --("no-fork") requires[start] "Does not fork the process when starting the daemon; this generally means that it will be tied to the shell that starts it")
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
                    daemon::daemon(!sub_app.is_present("no_fork"))?;
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

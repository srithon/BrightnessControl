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
        (@setting ArgsNegateSubcommands)
        (version: crate_version!())
        (author: "Sridaran Thoniyil")
        (about: "BrightnessControl is an XRandr interface which allows users to make relative brightness adjustments easily.")
        (@group monitor_override =>
            (@arg monitor: -m --monitor +takes_value value_name[ADAPTER_NAME] "Apply brightness changes to a specific display")
            (@arg active: --active "Apply brightness changes to the \"active\" monitor; note that this is \"active\" monitor is specific to BrightnessControl and has nothing to do with mouse location or keyboard focus")
            (@arg all: -a --all "Apply brightness changes to ALL connected monitors")
        )
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
            (@arg terminate_fade: -t --("terminate-fade") required_unless[action] "Terminates the current fade if one is currently running; this can be combined with one one of the brightness changing actions")
        )
        (@subcommand monitors =>
            (about: "Holds commands that control BrightnessControl behavior for multiple monitors")
            (visible_alias: "m")
            (@group action =>
                (@arg set_active: -s --("set-active") +takes_value value_name[monitor_adapter_name] "Sets the active monitor for use with \"brightness --active\"; use \"--get displays\" to see options")
            )
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
        (@subcommand get => 
            (about: "Retrieves values from the daemon")
            (visible_alias: "g")
            (@group action =>
                (@arg brightness: -b --brightness "Returns a newline-separated list of the brightness levels for all the connected displays OR all the monitors matched by the monitor override argument")
                (@arg mode: -m --mode "Returns 1 if the nightlight is on and 0 if it is off")
                (@arg displays: -d --displays "Returns a space-separated list of all the connected displays OR all the monitors matched by the monitor override argument")
                (@arg fading: -f --fading "Returns a newline-separated list of all the connected displays OR all the monitors matched by the monitor override argument")
                (@arg config: -c --config "Returns the active configuration in use by the daemon")
            )
        )
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
                    daemon::start_daemon(!sub_app.is_present("no_fork"))?;
                }
            },
            ("config", Some(sub_app)) => {
                if sub_app.is_present("print-default") {
                    println!("{}", daemon::config::persistent::CONFIG_TEMPLATE);
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

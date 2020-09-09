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
        (@setting AllowNegativeNumbers)
        (@setting VersionlessSubcommands)
        (version: crate_version!())
        (author: "Sridaran Thoniyil")
        (about: "BrightnessControl is an XRandr interface which allows users to make relative brightness adjustments easily.")
        (@subcommand brightness =>
            (about: "Holds commands involving brightness modification")
            (@group action =>
                (@attributes +required)
                (@arg increment: -i --increment +takes_value {percentage_validator} "Increases the current brightness by %")
                (@arg decrement: -d --decrement +takes_value {percentage_validator} "Decrements the current brightness by %")
                (@arg set: -s --set +takes_value {percentage_validator} "Sets the current brightness to %")
            )
        )
    )
}

fn main() -> Result<()> {
    let cli = get_cli_interface();

    let matches = cli.get_matches();


    Ok(())
}

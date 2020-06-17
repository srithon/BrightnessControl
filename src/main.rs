use directories::ProjectDirs;

use std::fs::{self, File};
use std::io::Error;

fn main() -> Result<(), Error> {
    println!("Hello, world!");
    // step 1
    // check if redshift mode or xrandr mode
    let project_directory = ProjectDirs::from("", "Sridaran Thoniyil", "BrightnessControl");
    // did not use if let because it would require the entire function to be indented

    if let None = project_directory {
        panic!("Cannot find base directory");
    }

    let project_directory = project_directory.unwrap();
    // cache the mode
    let cache_directory = project_directory.cache_dir();

    if !cache_directory.exists() {
        fs::create_dir_all(cache_directory)?;
    }

    println!("{}", cache_directory.to_str().unwrap());
    Ok(())
}

use directories::ProjectDirs;

fn main() {
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

    println!("{}", cache_directory.to_str().unwrap());
}

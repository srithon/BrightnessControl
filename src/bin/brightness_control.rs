use getopts::Options;
use directories::ProjectDirs;

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Error, ErrorKind, Seek, SeekFrom, Write, Read, Result};
use std::fmt::Display;
use std::process::Command;

use std::cmp;

enum DataValidatorResult<T> {
    Valid(T),
    Changed(T),
}

enum BrightnessChange {
    Adjustment(i8),
    Set(u8)
}

struct ProgramInput {
    brightness: Option<BrightnessChange>,
    configure_display: bool,
    toggle_nightlight: bool
}

struct ProgramState<'a> {
    cache_directory: &'a std::path::Path,
    file_open_options: OpenOptions,
    program_input: ProgramInput
}

use DataValidatorResult::*;

// where ....
fn get_valid_data_or_write_default<T>(file: &mut File, data_validator: &dyn Fn(&String) -> Result<DataValidatorResult<T>>, default_value: T) -> Result<T>
    where T: Display {
    let file_contents = {
        // wrapping in a closure allows the inner else and the
        // outer else clauses to share the same code
        // here, we want to return None if the file does not exist
        // or if the file's contents are not readable as a number
        (|| -> std::io::Result<DataValidatorResult<T>> {
            let mut buffer: Vec<u8> = Vec::new();
            file.read_to_end(&mut buffer)?;

            let string = unsafe {
                String::from_utf8_unchecked(buffer)
            };

            data_validator(&string)
        })()
    };

    if let Ok(Valid(contents)) = file_contents {
        Ok(contents)
    }
    else {
        file.seek(std::io::SeekFrom::Start(0))?;
        let new_value = {
            if let Ok(Changed(new_value)) = file_contents {
                new_value
            }
            else {
                default_value
            }
        };

        let formatted_new_value = format!("{}", new_value);
        // <<NOTE>> this can overflow? len() returns a usize
        file.set_len(formatted_new_value.len() as u64)?;

        write!(file, "{}", formatted_new_value)?;
        Ok(new_value)
    }
}

pub fn get_project_directory() -> Result<directories::ProjectDirs> {
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

    Ok(project_directory)
}

// 0 for regular
// 1 for night light
fn get_mode(program_state: &ProgramState) -> Result<u8> {
    let mode_filepath = program_state.cache_directory.join("mode");

    let mut mode_file = program_state.file_open_options.open(mode_filepath)?;
    mode_file.set_len(1)?;

    let toggled_mode: Option<u8> = {
        if program_state.increment == 0 && program_state.argument.eq("--toggle") {
            Some((|| -> Result<u8> {
                // toggle code
                let mut char_buffer: [u8; 1] = [0; 1];

                mode_file.read_exact(&mut char_buffer)?;

                let new_mode = {
                    match char_buffer[0] as char {
                        '0' => 1,
                        _   => 0
                    }
                };

                mode_file.seek(SeekFrom::Start(0))?;
                write!(mode_file, "{}", new_mode)?;
                return Ok(new_mode);
            })()?)
        }
        else {
            None
        }
    };

    if let Some(mode) = toggled_mode { 
        #[cfg(feature = "redshift")]
        {
            match mode {
                0 => {
                    // turn off redshift
                    let mut redshift_disable = Command::new("redshift");
                    redshift_disable.arg("-x");
                    redshift_disable.spawn()?;
                },
                1 => {
                    // turn on redshift
                    let mut redshift_enable = Command::new("redshift");
                    redshift_enable.arg("-O");
                    redshift_enable.arg("1400");
                    redshift_enable.spawn()?;
                },
                _ => panic!("Mode is {}!?", mode)
            };
        }

        Ok(mode)
    }
    else {
        get_valid_data_or_write_default(&mut mode_file, &| data_in_file: &String | {
            if let Ok(num) = data_in_file.parse::<u8>() {
                if num == 0 || num == 1 {
                    return Ok(Valid(num));
                }
            }

            return Err(Error::new(ErrorKind::InvalidData, "Invalid mode"));

        }, 0)
    }
}

fn configure_displays(displays_file: &mut std::fs::File) -> Result<Vec<String>> {
    let mut xrandr_current = Command::new("xrandr");
    xrandr_current.arg("--current");
    let command_output = xrandr_current.output()?;
    // the '&' operator dereferences ascii_code so that it can be compared with a regular u8
    // its original type is &u8
    let output_lines = command_output.stdout.split(| &ascii_code | ascii_code == '\n' as u8);
    let connected_displays: Vec<String> = output_lines.filter(| line | {
        // panic if the output is not UTF 8
        let line_as_string = std::str::from_utf8(line).unwrap();
        /*
           this predicate filters out everything but the first line
           example output
           eDP-1 connected primary 1920x1080+0+0 (normal left inverted right x axis y axis) 344mm x 193mm
           HDMI-1 disconnected (normal left inverted right x axis y axis)
           DP-1 disconnected (normal left inverted right x axis y axis)
           HDMI-2 disconnected (normal left inverted right x axis y axis)*
           */
        line_as_string.contains(" connected")
    }).map(| line | {
        // if it passed through the filter, it has to be valid UTF8
        // therefore, this unsafe call can be made safely
        let line_as_string = unsafe { std::str::from_utf8_unchecked(line) };
        // panic if the output does not contain a space
        // it has to contain a space because the filter predicate specifically has a space
        // in it
        // create a slice of the first Word
        line_as_string[0..line_as_string.find(' ').unwrap()].to_owned()
    }).collect();

    // sum the lengths of each display name, and then add (number of names - 1) to account
    // for newline separators between each name
    // subtract 1 because there is no newline at the end
    let displays_file_length = (connected_displays.len() - 1) +
        connected_displays.iter().map(| display_name | display_name.len()).sum::<usize>();

    // .iter() so that connected_displays is not moved
    for display in connected_displays.iter() {
        writeln!(displays_file, "{}", display)?;
    }

    // the above loop appends a newline to each display, including the last one
    // however, this call to set_len() cuts out this final newline
    displays_file.set_len(displays_file_length as u64)?;

    Ok(connected_displays)
}

fn get_displays(program_state: &ProgramState, force_reconfigure: bool) -> Result<Vec<String>> {
    let displays_filepath = program_state.cache_directory.join("displays");

    let mut displays_file = program_state.file_open_options.open(displays_filepath)?;

    if force_reconfigure || program_state.argument.eq("--configure-display") {
        configure_displays(&mut displays_file)
    }
    else {
        let buffered_display_file_reader = BufReader::new(&mut displays_file);
        // filter out all invalid lines and then collect them into a Vec<String>
        let read_displays = buffered_display_file_reader.lines().filter_map(| line | line.ok()).collect::<Vec<String>>();

        if read_displays.len() > 0 {
            Ok(read_displays)
        }
        else {
            configure_displays(&mut displays_file)
        }
    }
}

fn get_brightness(program_state: &ProgramState) -> Result<i16> {
    let brightness_filepath = program_state.cache_directory.join("brightness");

    let mut brightness_file = program_state.file_open_options.open(brightness_filepath)?;

    get_valid_data_or_write_default(&mut brightness_file, &| data_in_file: &String | {
        // need to trim this because the newline character breaks the parse
        if let Ok(num) = data_in_file.trim_end().parse::<i16>() {

            if num >= 0 {
                // ensure range of [0, 100]
                // <<NOTE>> weird behavior incase of overflow
                let new_num = cmp::max(cmp::min(program_state.increment + num, 100), 0);

                if num == new_num {
                    return Ok(Valid(num));
                }
                else {
                    return Ok(Changed(new_num));
                }
            }
        }

        return Err(Error::new(ErrorKind::InvalidData, "Invalid brightness"));
    }, 100)
}

fn create_xrandr_command(displays: Vec<String>, brightness: &String, _mode: u8) -> Command {
    let mut xrandr_call = Command::new("xrandr");

    for display in displays {
        xrandr_call.arg("--output");
        xrandr_call.arg(display);
    }

    xrandr_call.arg("--brightness")
        .arg(&brightness);

    #[cfg(not(feature = "redshift"))]
    {
        if _mode == 1 {
            xrandr_call.arg("--gamma")
                .arg("1.0:0.7:0.45");
        }
    }

    xrandr_call
}

fn get_cli_interface() -> Options {
    let mut options = Options::new();
    options
        .parsing_style(getopts::ParsingStyle::FloatingFrees)
        .optflag("h", "help", "Prints the help menu")
        .optflag("c", "configure-display", "Uses the current display configuration for future calls to BrightnessControl")
        .optflag("t", "toggle-nightlight", "Toggles the nightlight mode on/off")
        .optopt("s", "set", "Sets the current brightness to some percentage [0..100]", "PERCENTAGE")
        .optopt("i", "increment", "Increments the current brightness by some (integer) percentage. It can be negative", "PERCENTAGE")
        .optopt("d", "decrement", "Decrements the current brightness by some (integer) percentage. It can be positive", "PERCENTAGE");
    options
}

fn main() -> Result<()> {
    // step 1
    // check if redshift mode or xrandr mode
    let project_directory = get_project_directory()?;
    let cache_directory = project_directory.cache_dir();

    let file_open_options = {
        let mut file_open_options = OpenOptions::new();
        file_open_options.read(true);
        file_open_options.write(true);
        file_open_options.create(true);
        file_open_options
    };

    let cli = get_cli_interface();
    let args = std::env::args().collect::<Vec<String>>();

    // this lets you see exactly how the user invoked the program
    // this is then used in the help screen
    let program_invocation_name = args[0].clone();

    let matches = match cli.parse(&args[1..]) {
        Ok(matches) => matches,
        Err(error) => panic!(error.to_string())
    };

    if matches.opt_present("help") {
        let brief = format!("Usage: {} [options]", program_invocation_name);
        print!("{}", cli.usage(&brief));
        return Ok(());
    }

    let brightness = {
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

        result?
    };

    let program_input = ProgramInput {
        brightness,
        configure_display: matches.opt_present("configure-display"),
        toggle_nightlight: matches.opt_present("toggle-nightlight")
    };

    let program_state = ProgramState {
        cache_directory,
        file_open_options,
        program_input
    };

    let mode = get_mode(&program_state)?;
    let displays = get_displays(&program_state, false)?;
    let brightness = get_brightness(&program_state)?;

    let brightness_string = format!("{:.2}", brightness as f32 / 100.0);
    println!("Brightness: {}", brightness_string);

    let mut xrandr_call = create_xrandr_command(displays, &brightness_string, mode);

    // this variable is only used if the "auto-reconfigure" feature is enabled
    let mut _call_handle = xrandr_call.spawn()?;

    #[cfg(feature = "auto-reconfigure")]
    {
        let exit_status = _call_handle.wait()?;

        // if the call fails, then the configuration is no longer valid
        // reconfigures the display and then tries again
        if !exit_status.success() {
            println!("Reconfiguring!");
            // force reconfigure
            let displays = get_displays(&program_state, true)?;
            create_xrandr_command(displays, &brightness_string, mode).spawn()?;
        }
    }

    Ok(())
}

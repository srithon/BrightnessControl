use directories::ProjectDirs;

use tokio::{
    fs::{self, File, OpenOptions},
    io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader},
};

use std::io::{Error, ErrorKind, Result};

use std::path::Path;

use std::fmt::Display;

use super::config::{
    persistent::{DaemonOptions, CONFIG_TEMPLATE},
    runtime::CachedState,
};

use crate::shared::MonitorOverrideTOMLCompatible;

pub type ConfigAttempt = std::result::Result<DaemonOptions, toml::de::Error>;

pub struct FileUtils {
    pub project_directory: ProjectDirs,
    pub file_open_options: OpenOptions,
}

impl FileUtils {
    pub fn new() -> Result<FileUtils> {
        let project_directory = get_project_directory()?;

        let file_open_options = {
            let mut file_open_options = OpenOptions::new();
            file_open_options.read(true);
            file_open_options.write(true);
            file_open_options.create(true);
            file_open_options
        };

        Ok(FileUtils {
            project_directory,
            file_open_options,
        })
    }

    fn get_cache_dir(&self) -> &Path {
        self.project_directory.cache_dir()
    }

    pub fn get_daemon_pid_file(&self) -> std::path::PathBuf {
        self.get_cache_dir().join("daemon.pid")
    }

    // returns the file and whether or not it existed prior to opening it
    pub async fn open_configuration_file(&self) -> Result<(File, bool)> {
        let config_dir = self.project_directory.config_dir();
        let filepath = config_dir.join("config.toml");

        if !config_dir.exists() {
            fs::create_dir(self.project_directory.config_dir()).await?;
        }

        let file_exists = fs::metadata(&filepath).await.is_ok();
        Ok((self.file_open_options.open(filepath).await?, file_exists))
    }

    async fn open_cache_file_with_options(
        &self,
        file_name: &str,
        open_options: &OpenOptions,
    ) -> Result<File> {
        let filepath = self.project_directory.cache_dir().join(file_name);
        open_options.open(filepath).await
    }

    async fn open_cache_file(&self, file_name: &str) -> Result<File> {
        self.open_cache_file_with_options(file_name, &self.file_open_options)
            .await
    }

    async fn get_central_cache_file(&self) -> Result<File> {
        self.open_cache_file("persistent_state.toml").await
    }

    pub async fn get_cached_state(&self) -> Result<CachedState> {
        let mut cache_file = self.get_central_cache_file().await?;

        const INITIAL_BUFFER_SIZE: usize = 1024;

        let mut file_contents_buffer = Vec::with_capacity(INITIAL_BUFFER_SIZE);

        // fill buffer
        if let Err(e) = cache_file.read_to_end(&mut file_contents_buffer).await {
            eprintln!("Failed to read from configuration file! {e}");
            return Err(e);
        }

        let state = {
            match toml::from_str::<CachedState>(
                std::str::from_utf8(&file_contents_buffer)
                    .map_err(|e| Error::new(ErrorKind::InvalidData, e))?,
            ) {
                Ok(state) => {
                    // validate values
                    if !state.validate() {
                        CachedState::default()
                    } else {
                        state
                    }
                }
                _ => {
                    eprintln!("Couldn't parse persistent state file!");
                    CachedState::default()
                }
            }
        };

        Ok(state)
    }

    pub async fn write_cached_state(&self, cached_state: &CachedState) -> Result<()> {
        let mut cache_file = self.get_central_cache_file().await?;

        match toml::ser::to_string(cached_state) {
            Ok(serialized_toml) => {
                // write state to file
                cache_file.write(&serialized_toml.as_bytes()).await?;
                // cut off excess bytes from previous state of file
                cache_file.set_len(serialized_toml.len() as u64).await?;
            }
            Err(e) => {
                eprintln!("Failed to serialize cached state: {e}");
                return Err(std::io::Error::new(ErrorKind::Other, format!("{e}")));
            }
        };

        Ok(())
    }

    // checks if the header in the config template reads a different version
    // than the current version of BrightnessControl
    // if they do not match OR anything goes wrong during the check, overwrites
    // the template with CONFIG_TEMPLATE
    pub async fn update_config_template(&self) -> Result<()> {
        // read header of current template
        let data_dir = self.project_directory.data_dir();

        if !data_dir.exists() {
            fs::create_dir_all(data_dir).await?;
        }

        let template_filepath = data_dir.join("config_template.toml");
        let template_exists = template_filepath.exists();

        let mut template_file = self.file_open_options.open(&template_filepath).await?;
        let mut template_file_clone = template_file.try_clone().await?;

        // this allows us to handle any errors in this section the same way
        let overwrite_template_result = (|| async move {
            if template_exists {
                // we only have to read until the first newline
                // # a.b.c
                // 12345678
                // 2 bytes extra incase 'a' becomes double(/triple)-digits
                const BYTES_TO_READ: usize = 20;

                let buffered_reader = BufReader::with_capacity(BYTES_TO_READ, &mut template_file);

                // buffered_reader.fill_buf().await?;

                if let Ok(Some(first_line)) = buffered_reader.lines().next_line().await {
                    const NUM_CHARS_TO_IGNORE: usize = "# v".len();
                    // Strings from BufReader::lines do not include newlines at the
                    // end
                    let version_string = &first_line[NUM_CHARS_TO_IGNORE..];

                    let current_version_string = {
                        let beginning_trimmed = &CONFIG_TEMPLATE[NUM_CHARS_TO_IGNORE..];
                        let newline_index = beginning_trimmed.find('\n').unwrap();
                        &beginning_trimmed[..newline_index]
                    };

                    // compare to actual version string
                    if version_string.eq(current_version_string) {
                        return Ok(());
                    } else {
                        println!(
                            "Config template updated! \"{version_string}\" to \"{current_version_string}\""
                        );
                    }
                }
            } else {
                println!("Config template saved to {}", &template_filepath.display());
            }

            // the cause of this error is irrelevant, so it doesnt need a
            // message
            Err(Error::new(ErrorKind::Other, ""))
        })()
        .await;

        // dont care about the cause
        if overwrite_template_result.is_err() {
            // overwrite
            overwrite_file_with_content(&mut template_file_clone, CONFIG_TEMPLATE).await?;
        }

        Ok(())
    }
}

pub fn get_project_directory() -> Result<directories::ProjectDirs> {
    let project_directory = ProjectDirs::from("", "Sridaran Thoniyil", "BrightnessControl");
    // did not use if let because it would require the entire function to be indented

    if project_directory.is_none() {
        panic!("Cannot find base directory");
    }

    let project_directory = project_directory.unwrap();
    // cache the mode
    let cache_directory = project_directory.cache_dir();

    if !cache_directory.exists() {
        std::fs::create_dir_all(cache_directory)?;
    }

    Ok(project_directory)
}

pub async fn get_configuration_from_file(configuration_file: &mut File) -> Result<DaemonOptions> {
    // 8 KB
    const INITIAL_BUFFER_SIZE: usize = 8 * 1024;

    let mut configuration_buffer = Vec::with_capacity(INITIAL_BUFFER_SIZE);

    // fill buffer
    if let Err(e) = configuration_file
        .read_to_end(&mut configuration_buffer)
        .await
    {
        eprintln!("Failed to read from configuration file! {e}");
    }

    macro_rules! require {
        ( $x:expr ) => {{
            $x.map_err(|e| Error::new(ErrorKind::InvalidData, e))?
        }};
    }

    let parsed_toml: toml::Value = require!(toml::from_str(require!(std::str::from_utf8(
        &configuration_buffer
    ))));

    let mut config = DaemonOptions::default();

    macro_rules! overwrite_values {
        ( $( $x:ident ),* ) => {
            {
                $(
                    if let Some(option) = parsed_toml.get(stringify!($x)) {
                        config.$x = require!(option.clone().try_into());
                    }
                )*
            }
        };
    }

    // TODO figure out how to use derive macro for this
    overwrite_values!(
        use_redshift,
        auto_remove_displays,
        fade_options,
        nightlight_options
    );

    if let Some(monitor_default_behavior) = parsed_toml.get("monitor_default_behavior") {
        // we have to hardcode this case because otherwise, it will attempt to directly parse it
        // into a MonitorOverride rather than using the MonitorOverrideTOMLCompatible intermediate
        // it will not understand the internal tag field and will fail to parse
        config.monitor_default_behavior = require!(monitor_default_behavior
            .clone()
            .try_into::<MonitorOverrideTOMLCompatible>())
        .into();
    }

    Ok(config)
}

pub async fn overwrite_file_with_content<T>(file: &mut File, new_content: T) -> Result<()>
where
    T: Display,
{
    file.seek(std::io::SeekFrom::Start(0)).await?;

    let formatted_new_content = format!("{new_content}");

    // <<NOTE>> this can overflow? len() returns a usize
    file.set_len(formatted_new_content.len() as u64).await?;

    file.write_all(formatted_new_content.as_bytes()).await?;

    Ok(())
}

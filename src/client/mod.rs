pub mod core;

pub fn handle_input(matches: &clap::ArgMatches) -> std::io::Result<()> {
    core::handle_input(matches)
}

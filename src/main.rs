mod app;
mod editor;
mod runner;

use std::{error::Error, process::ExitCode};

use clap::{Parser, ValueEnum};

#[derive(Parser, Debug)]
#[command(
    name = "collapse",
    version,
    about = "Run commands sequentially, then browse their output as tabs",
    after_long_help = "Each COMMAND is a shell command, so quote commands that contain arguments.\n\nExamples:\n  collapse 'cargo test' 'cargo clippy --all-targets'\n  collapse 'npm run lint' 'npm test' 'npm run build'\n  collapse --color auto 'git status' 'git log --oneline -20'"
)]
struct Cli {
    /// When to ask child commands to emit colored output
    #[arg(long, value_enum, default_value_t = CliColor::Always)]
    color: CliColor,

    /// Shell commands to review and run in order; omit to open an empty command editor
    #[arg(value_name = "COMMAND", num_args = 0.., allow_hyphen_values = true)]
    commands: Vec<String>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum CliColor {
    /// Force colors from common tools (the default)
    Always,
    /// Let each command decide; piped commands often disable colors
    Auto,
    /// Ask commands not to emit colors
    Never,
}

impl From<CliColor> for runner::ColorMode {
    fn from(value: CliColor) -> Self {
        match value {
            CliColor::Always => Self::Always,
            CliColor::Auto => Self::Auto,
            CliColor::Never => Self::Never,
        }
    }
}

fn main() -> Result<ExitCode, Box<dyn Error>> {
    let cli = Cli::parse();
    let exit_code = ratatui::run(|terminal| app::run(terminal, cli.commands, cli.color.into()))?;
    Ok(ExitCode::from(exit_code))
}

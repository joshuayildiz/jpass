mod app;
mod components;
mod import;
mod install;
mod model;
mod pwgen;
mod vault;
mod view;

use app::Remote;

/// age-encrypted CSV password manager
#[derive(clap::Parser)]
#[command(
    version,
    after_help = "File: ~/pCloudDrive/pass.rage, on macOS ~/pCloud Drive/pass.rage (created on first run if missing)\n\n\
                  Built-in global hotkey (macOS / X11): Cmd+Space"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(clap::Subcommand)]
enum Cmd {
    /// Show/hide the running instance, or start one (the default; bind this to a global shortcut)
    Toggle,
    /// Show the running instance
    Show,
    /// Start in the background without a window (used for autostart)
    Hidden,
    /// Merge entries from a CSV file (plaintext or age-encrypted) into the vault, no duplicates
    Import {
        /// CSV file to import
        file: String,
    },
    /// (Re)install as an app + start at login (also done on first run)
    Install,
}

fn main() -> eframe::Result {
    let cli = <Cli as clap::Parser>::parse();
    match cli.command.unwrap_or(Cmd::Toggle) {
        Cmd::Import { file } => match import::main(&file) {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("jpass: {e}");
                std::process::exit(1);
            }
        },
        Cmd::Install => match install::main() {
            Ok(()) => Ok(()),
            Err(e) => {
                eprintln!("jpass: install failed: {e}");
                std::process::exit(1);
            }
        },
        Cmd::Toggle => app::run(Remote::Toggle, false),
        Cmd::Show => app::run(Remote::Show, false),
        Cmd::Hidden => app::run(Remote::Toggle, true),
    }
}

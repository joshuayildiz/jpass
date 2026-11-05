mod app;
mod components;
#[cfg(not(target_os = "macos"))]
mod desktop;
mod import;
mod install;
mod launcher;
mod model;
mod pwgen;
mod vault;
mod view;

use app::Remote;

fn main() -> eframe::Result {
    let cli = <Cli as clap::Parser>::parse();
    match cli.command.unwrap_or(Cmd::Toggle) {
        Cmd::Toggle => app::run(Remote::Toggle, false),
        Cmd::Show => app::run(Remote::Show, false),
        Cmd::Hidden => app::run(Remote::Toggle, true),
        Cmd::Import { file } => finish(import::main(&file)),
        Cmd::Install => finish(install::main().map_err(|e| format!("install failed: {e}"))),
    }
}

/// age-encrypted CSV password manager and application launcher
#[derive(clap::Parser)]
#[command(
    version,
    after_help = "File: ~/pCloudDrive/pass.rage, on macOS ~/pCloud Drive/pass.rage (created on first run if missing)\n\n\
                  Global hotkey: Cmd+Space on macOS, Alt+Space on Linux (a KDE shortcut, or an X11 grab)"
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

/// End a command-line subcommand: report an error and set the exit code.
fn finish(result: Result<(), String>) -> ! {
    if let Err(e) = result {
        eprintln!("jpass: {e}");
        std::process::exit(1);
    }
    std::process::exit(0)
}

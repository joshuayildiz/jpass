# jpass — status

## Done

- [X] Stack chosen: single Rust binary, `eframe`/`egui` GUI + `age` crate (rage-compatible), no webview/Node
- [X] Encrypted vault `~/pCloudDrive/pass.rage` (`~/pCloud Drive/pass.rage` on macOS), passphrase, age format, fixed path, no env vars
- [X] Decrypted content stays a plain CSV (`Account,Identifier,Password`); extra columns and existing header are preserved
- [X] File is created on first run if missing (passphrase asked twice)
- [X] Safe saving: encrypt → verify round-trip → temp file → atomic rename; previous version kept as `pass.rage.bak`
- [X] Refuses to overwrite if pCloud synced a newer `pass.rage` (offers reload); auto-reloads on show if the file changed
- [X] Fixed scrypt work factor (20) so files stay decryptable by the `rage` CLI (the auto-calibrated value on this CPU exceeded rage's limit of 22); decrypt accepts up to 26
- [X] Spotlight-style popup: borderless, centered, square-cornered, grows with content, keyboard-first
- [X] Follows system light/dark theme (KDE `kdeglobals`, GNOME `gsettings`, macOS `defaults`), re-checked on every show
- [X] Search + keys: `↵` copy password, `Ctrl+U` copy identifier, `Ctrl+R` reveal, `Ctrl+E` edit, `Ctrl+D` delete, `Ctrl+N` new, `Ctrl+L` lock, `Esc` step back / hide
- [X] Clipboard auto-clears after 20 s
- [X] Vault re-locks after the window has been hidden for 5 min
- [X] Renamed "Email/Username/ID" → "Identifier" in the UI and in newly created files
- [X] Single instance: `jpass toggle` / `show` / `hidden` talk to the running instance over a unix socket
- [X] Linux runs through XWayland so hide/show really works (winit can't hide windows on Wayland)
- [X] Global hotkey in-app: Cmd+Space (macOS: Command; Linux: Alt+Space, which is the Cmd key on this keyboard) — you confirmed it works
- [X] `Enter` on an entry copies the password **and** closes the window
- [X] Window hides when it loses focus (click anywhere else). Root cause was `ViewportCommand::Focus` being dropped while the window was still mapping; now retried until focus is granted
- [X] `jpass import <file>`: merges a CSV into the vault without duplicates (account + identifier + password); works for plaintext **and** passphrase-encrypted import files (separate passphrase, Enter = same as master)
- [X] First-run setup / `jpass install`: Linux → `~/.local/bin/jpass`, launcher in `~/.local/share/applications`, XDG autostart (starts hidden at login); macOS → `/Applications/jpass.app` (falls back to `~/Applications`) + LaunchAgent
- [X] Saved file is always sorted (account, then identifier, case-insensitive; header stays first)
- [X] Unit tests for CSV round-trip, encryption, conflict detection and import merge (`cargo test --release`)
- [X] Single-instance check on every launch (incl. `hidden` autostart): the socket is claimed atomically before any window is created
- [X] Waits for pCloud to mount (FUSE) instead of offering to create a new `pass.rage`; re-checks the file every second while locked and on every show
- [X] Redesign in the spirit of elm-lang.org / acadia.engineering: IBM Plex Sans + Mono (bundled, OFL), square corners, 1px border + hard offset shadow, Elm blue accent, Acadia grays for dark mode, quiet text links, outlined buttons that invert on hover
- [X] Every field is trimmed (parse, import, UI edits); the parser tolerates padding and quoted fields after spaces
- [X] Saved file is column-aligned for monospace editors (sorted, trimmed, padded after the comma); `csv` crate dropped
- [X] If the IPC socket cannot be created jpass keeps running instead of exiting silently
- [X] Strong readable password generator (Apple style `bokdal-Tefmuz-9sanhr`, ~74 bits, no look-alike characters, OS CSPRNG); new entries start with one filled in and visible; "generate" link / `Ctrl+G` rerolls it
- [X] Spacing/alignment pass (measured with pixel crops): one 28px gutter on both sides, one text column shared by the search text and the row text, search bar and separator now identical on every screen (the list one used to sit 12px too high), glyph optically centred on the text, no dead space under the footer, revealed row grows to fit its third line, error banner is a fixed 48px, and panel heights of the lock/create/edit/delete screens are measured instead of guessed (the create hint row and the edit buttons used to be clipped)
- [X] macOS support (built and run on a Mac, works): hidden window can be shown again (`App::logic` handles commands while hidden), focus returns to the previous app (`NSApp.hide`/`unhide`), pCloud folder name per platform, `Modifiers::META` for Cmd
- [X] MIT license (`LICENSE`, `license = "MIT"` in `Cargo.toml`; README lists the third-party licenses)
- [X] Built, installed to `~/.local/bin/jpass` and started in the background (disowned); Linux re-checked after the macOS changes (build, 11 tests, Alt+Space focuses the keyboard)

## Not verified / open

- [ ] First-run setup was tested in a throwaway `$HOME`; on your real account it wrote only `~/.local/share/applications/jpass.desktop`, `~/.config/autostart/jpass.desktop` and `~/.config/jpass/installed-v1`
- [X] KRunner launcher shortcut (Alt+Space / Alt+F2 / Search key) disabled in `~/.config/kglobalshortcutsrc` at your request (`[services][org.kde.krunner.desktop] _launch=none`); takes effect after re-login
- [X] KDE: jpass registers `Alt+Space` for `jpass.desktop` itself at start (kglobalaccel over D-Bus, `SetPresent` only, so a key chosen in System Settings wins) and skips the X11 grab there. The compositor consumes the key: the focused app (e.g. Firefox) no longer receives Alt+Space, and there is no double toggle
- [X] Window shown via `jpass toggle` (no key press of its own) could be left "demands attention": after showing, jpass now asks KWin (script) / xdotool to activate it
- [ ] Dark theme has never been rendered (the Linux desktop is light)
- [ ] Not implemented (not requested): changing the master passphrase, entry notes/URL fields, TOTP

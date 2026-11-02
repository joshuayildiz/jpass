# jpass

A small desktop password manager with a Spotlight-style popup.

Press a global shortcut, type a few letters, press **Enter**: the password is on
your clipboard and the window is gone. That is the whole workflow.

Your passwords live in **one age-encrypted file**, `~/pCloudDrive/pass.rage`
(`~/pCloud Drive/pass.rage` on macOS, where pCloud mounts under that name).
jpass writes nothing else to disk except that file (and a `.bak` of the previous
version next to it), so syncing the folder (here: pCloud) is all the backup and
multi-device story there is.

Linux (tested on KDE/Wayland, CachyOS) and macOS (tested).

## What it is for

- Looking up and copying credentials quickly, from the keyboard, from anywhere.
- Keeping the vault as a plain, inspectable, rage-compatible file: you can always
  decrypt it without jpass using `rage -d pass.rage` (or `age -d`) on the file in your pCloud folder.
- Generating strong, readable passwords (Apple style, e.g. `sichan-2eMhim-sogkad`).

It is a personal tool, not a team vault: one passphrase, one file, no accounts, no network code.

## Using it

| Key | Action |
| --- | --- |
| `Alt+Space` (Linux, X11/XWayland) / `Cmd+Space` (macOS) | show / hide jpass (global) |
| type | filter by account or identifier |
| `↑` `↓` | move selection |
| `Enter` | copy the password and close |
| `Ctrl+U` | copy the identifier |
| `Ctrl+R` | reveal / hide the password |
| `Ctrl+E` / `Ctrl+D` / `Ctrl+N` | edit / delete / new entry |
| `Ctrl+G` (in the form) | generate a new password |
| `Ctrl+L` | lock |
| `Esc` | clear search, then hide |

The window also hides when it loses focus. After 5 minutes hidden, the vault locks again.
The clipboard is cleared 20 seconds after a copy.

On Wayland the compositor does not let an application grab keys by itself. The built-in
shortcut works through XWayland; the dependable alternative is to bind the command
`jpass --toggle` to a key in your desktop's shortcut settings. (KDE may refuse to give the
keyboard to a window that was shown without a key press behind it, for example by a script
running `jpass --toggle`; the window then just blinks in the task bar.)

### Command line

```
jpass                 start, or show/hide the running instance
jpass --toggle        show/hide the running instance (bind this to a shortcut)
jpass --show          show the running instance
jpass --hidden        start in the background without a window (used for autostart)
jpass --import FILE   merge entries from a CSV into the vault, skipping duplicates
jpass --install       install as an app and start at login (also done on first run)
```

Only one instance runs at a time; launching `jpass` again talks to the running one.

### Importing

`jpass --import FILE` accepts a CSV in the same format as the vault, either plaintext or
age-encrypted with its own passphrase (press Enter at the second prompt to reuse the master
passphrase). An entry is a duplicate when account, identifier and password all match
(account and identifier compared ignoring case). Same account + identifier with a
*different* password is kept as a second entry and reported. If FILE was plaintext,
removing it afterwards is up to you.

## The file format

`pass.rage` is a standard [age](https://age-encryption.org) file encrypted to a passphrase
(scrypt work factor 20). Decrypted, it is plain CSV, aligned so it reads well in a monospace editor:

```
Account, Identifier,       Password
GitHub,  me@example.com,   hunter2
Gmail,   joshua@gmail.com, s3cret!
```

- Fields are trimmed; fields containing a comma, quote or line break are double-quoted.
- Rows are kept sorted (account, then identifier, case-insensitive) every time the file is saved.
- Extra columns are preserved.
- Saving encrypts, verifies it can decrypt the result, writes a temp file and renames it over
  the vault, so a half-written file is never synced. If the file changed on disk meanwhile
  (another machine synced), jpass refuses to overwrite it and offers a reload.
- jpass waits for the pCloud folder to be mounted and never creates a vault in an unmounted folder.

## Security notes

- Passwords exist as plaintext only in memory while the vault is unlocked and in the clipboard
  for 20 seconds after a copy. Locking drops the data but does not overwrite entry memory, and
  memory is not locked against swapping (only the passphrase is wiped when dropped).
- Security rests on the master passphrase. Use a long one.
- Generated passwords use the operating system's random source, ≈74 bits of entropy.
- This project has had **no independent security review**.

## Build and install

Requirements: a recent stable Rust toolchain (the crate uses edition 2024) and `make`.
On Linux the usual X11/Wayland development libraries that `eframe` needs.

```
make                # build target/release/jpass
make test           # run the tests
make install        # build, install for the current user, restart the background instance
make uninstall      # remove what install created (never touches pass.rage)
```

`make install` runs `jpass --install`, which sets up everything per user (no root):

- **Linux:** `~/.local/bin/jpass`, a launcher in `~/.local/share/applications`, and an
  autostart entry in `~/.config/autostart` (starts hidden at login so the hotkey is always live).
- **macOS:** `/Applications/jpass.app` (or `~/Applications`) and a login LaunchAgent.

The same setup runs automatically the first time the binary is launched.

### Layout

```
src/main.rs      UI, single-instance socket, global hotkey, CLI
src/vault.rs     CSV parse/serialise (trim, align, sort), age encryption, atomic save, merge
src/pwgen.rs     password generator
src/install.rs   first-run / --install setup
assets/fonts/    IBM Plex Sans + Mono (bundled into the binary) and their license
```

Debug builds understand a few `JPASS_DEV_*` environment variables used for UI testing
(auto-unlock, jumping to a screen, disabling hide-on-blur). They are compiled out of release builds.

## Licensing

**jpass itself is MIT licensed** (see `LICENSE`). The MIT license is compatible with every
dependency below, including the Apache-2.0-only ones (`winit`, `glutin`, `rpassword`, ...).

**Bundled fonts: SIL Open Font License 1.1.** IBM Plex Sans (Regular, SemiBold) and IBM Plex
Mono (Regular), © 2017 IBM Corp., Reserved Font Name "Plex", are embedded in the binary and
stored in `assets/fonts/`. The OFL requires that the copyright notice and license text
(`assets/fonts/OFL.txt`) travel with the fonts, that the fonts are not sold on their own, and
that modified versions do not use the Reserved Font Name. The fonts here are unmodified.

**Dependencies.** About 530 crates (checked with `cargo metadata`); all are permissive
licenses (MIT, Apache-2.0, BSD, ISC, Zlib, Unicode-3.0, BSL-1.0, Unlicense, 0BSD) with these
exceptions worth knowing about:

- `option-ext` (via `dirs`): **MPL-2.0**. File-level copyleft; fine to link and ship as-is,
  but changes to that crate's own files must be shared under the MPL.
- `epaint_default_fonts` (egui's built-in fallback fonts, also embedded in the binary):
  **OFL-1.1** and **Ubuntu Font Licence 1.0** in addition to MIT/Apache; their notices must
  accompany binary distributions.
- `self_cell` (Apache-2.0 **OR** GPL-2.0) and `r-efi` (MIT/Apache **OR** LGPL): take the
  permissive option; nothing here requires the GPL/LGPL terms.

MIT, Apache-2.0 and BSD require the copyright notices and license texts of the dependencies
to be reproduced when you distribute **binaries**. Building from source (as `make` does) pulls
the crates and their licenses from crates.io, so nothing extra is needed. For a binary release,
generate a third-party notice file, for example with
[`cargo about`](https://github.com/EmbarkStudios/cargo-about) or
[`cargo license`](https://github.com/onur/cargo-license), and ship it next to the binary.

The age file format is an open specification; compatibility with the `age`/`rage` command-line
tools is intentional (the work factor is capped at 20, below their limit of 22).

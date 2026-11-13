// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Window icon for the AppImage on Wayland.
//!
//! Wayland has no window icon of its own: the compositor looks up the
//! desktop file named after the window's app id (`stintory`, GTK's program
//! name) and shows its `Icon`. The deb and rpm install that file; an
//! AppImage doesn't, so KDE shows the generic Wayland icon.
//!
//! When running as an AppImage, [`integrate`] writes a hidden desktop file
//! (`NoDisplay=true`, so no menu entry) and the logo to the user's data
//! directory before the window opens. A `stintory.desktop` that we didn't
//! write (e.g. from Gear Lever or a manual install) is left alone.

use std::io;
use std::path::{Path, PathBuf};

/// Must match the window's app id, see the module docs.
const DESKTOP_FILE: &str = "stintory.desktop";
/// Marks desktop files we wrote, so we only ever overwrite our own.
const MARKER: &str = "X-Stintory-AppImage=true";
const ICON: &[u8] = include_bytes!("../icons/128x128@2x.png");

/// Installs the hidden desktop file if we run as an AppImage. Errors are
/// logged: the only effect is the generic icon.
pub fn integrate() {
    let Some(appimage) = std::env::var_os("APPIMAGE").map(PathBuf::from) else {
        return;
    };
    let Some(data_home) = data_home() else {
        return;
    };
    if let Err(e) = install(&data_home, &appimage) {
        tracing::warn!(error = %e, "couldn't install the desktop file for the window icon");
    }
}

/// `$XDG_DATA_HOME`, or `~/.local/share`.
fn data_home() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
}

fn install(data_home: &Path, appimage: &Path) -> io::Result<()> {
    let icon = data_home.join("icons/hicolor/256x256/apps/stintory.png");
    write_if_changed(&icon, ICON)?;

    let desktop = data_home.join("applications").join(DESKTOP_FILE);
    match std::fs::read_to_string(&desktop) {
        Ok(existing) if !existing.lines().any(|l| l == MARKER) => return Ok(()),
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let entry = desktop_entry(appimage, &icon);
    write_if_changed(&desktop, entry.as_bytes())
}

fn desktop_entry(appimage: &Path, icon: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Stintory\n\
         Comment=Record and compare your sim racing laps\n\
         Exec={exec}\n\
         Icon={icon}\n\
         Terminal=false\n\
         NoDisplay=true\n\
         {MARKER}\n",
        exec = exec_arg(&appimage.to_string_lossy()),
        icon = escape_value(&icon.to_string_lossy()),
    )
}

/// Quotes a path for the `Exec` key (desktop entry spec, "The Exec key").
fn exec_arg(path: &str) -> String {
    let mut quoted = String::from("\"");
    for c in path.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    escape_value(&quoted)
}

/// Escapes a string value (desktop entry spec, "Possible value types").
fn escape_value(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
        .replace('\r', "\\r")
}

fn write_if_changed(path: &Path, contents: &[u8]) -> io::Result<()> {
    if std::fs::read(path).ok().as_deref() == Some(contents) {
        return Ok(());
    }
    let dir = path.parent().expect("has a parent");
    std::fs::create_dir_all(dir)?;
    let name = path.file_name().expect("has a file name").to_string_lossy();
    let tmp = dir.join(format!(".{name}.tmp"));
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_entry_and_icon() {
        let dir = tempdir();
        install(&dir, Path::new("/opt/My Apps/Stintory.AppImage")).unwrap();
        let entry = std::fs::read_to_string(dir.join("applications/stintory.desktop")).unwrap();
        assert!(entry.contains("Exec=\"/opt/My Apps/Stintory.AppImage\"\n"));
        assert!(entry.contains("NoDisplay=true\n"));
        let icon = dir.join("icons/hicolor/256x256/apps/stintory.png");
        assert!(entry.contains(&format!("Icon={}\n", icon.display())));
        assert_eq!(std::fs::read(icon).unwrap(), ICON);
    }

    #[test]
    fn updates_own_entry_but_not_foreign_one() {
        let dir = tempdir();
        let desktop = dir.join("applications/stintory.desktop");
        install(&dir, Path::new("/a/Stintory.AppImage")).unwrap();
        install(&dir, Path::new("/b/Stintory.AppImage")).unwrap();
        assert!(
            std::fs::read_to_string(&desktop)
                .unwrap()
                .contains("Exec=\"/b/")
        );

        std::fs::write(&desktop, "[Desktop Entry]\nExec=mine\n").unwrap();
        install(&dir, Path::new("/c/Stintory.AppImage")).unwrap();
        assert_eq!(
            std::fs::read_to_string(&desktop).unwrap(),
            "[Desktop Entry]\nExec=mine\n"
        );
    }

    #[test]
    fn quotes_exec() {
        assert_eq!(exec_arg("/x/a$b\"c"), r#""/x/a\\$b\\"c""#);
    }

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("stintory-appimage-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}

// SPDX-FileCopyrightText: 2026 The Stintory Contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The app and the server only sync when both run the same version: the sync
//! API has no compatibility guarantees between versions.

use std::cmp::Ordering;

use sy_model::api::UpdateNeeded;

use crate::SyncError;

/// Version of this client (the whole workspace shares one version).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Ok if the server runs the same version as this client.
pub fn check_version(server: &str) -> Result<(), SyncError> {
    if server == VERSION {
        return Ok(());
    }
    Err(SyncError::VersionMismatch {
        app: VERSION.to_owned(),
        server: server.to_owned(),
        update: update_needed(VERSION, server),
    })
}

/// Which side is older (and has to be updated).
fn update_needed(app: &str, server: &str) -> UpdateNeeded {
    match (release(app), release(server)) {
        (Some(a), Some(s)) => match a.cmp(&s) {
            Ordering::Less => UpdateNeeded::App,
            Ordering::Greater => UpdateNeeded::Server,
            // same release, different pre-release or build
            Ordering::Equal => UpdateNeeded::AppOrServer,
        },
        _ => UpdateNeeded::AppOrServer,
    }
}

/// `major.minor.patch` of a version like `0.3.1` or `0.4.0-beta.1`.
fn release(version: &str) -> Option<(u64, u64, u64)> {
    let core = version.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let v = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(v)
}

/// What the user has to do, for the UI.
pub fn mismatch_message(app: &str, server: &str, update: UpdateNeeded) -> String {
    let what = match update {
        UpdateNeeded::App => "Update the app to sync.",
        UpdateNeeded::Server => "Update the server to sync.",
        UpdateNeeded::AppOrServer => {
            "Update the app or the server so both are on the same version."
        }
    };
    format!("This app ({app}) and the server ({server}) run different versions. {what}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_older_side_needs_the_update() {
        assert_eq!(update_needed("0.3.1", "0.3.2"), UpdateNeeded::App);
        assert_eq!(update_needed("0.10.0", "0.9.9"), UpdateNeeded::Server);
        assert_eq!(update_needed("1.0.0", "0.99.0"), UpdateNeeded::Server);
        assert_eq!(
            update_needed("0.4.0-beta.1", "0.4.0"),
            UpdateNeeded::AppOrServer
        );
        assert_eq!(
            update_needed("0.4.0", "something"),
            UpdateNeeded::AppOrServer
        );
    }

    #[test]
    fn only_the_same_version_syncs() {
        assert!(check_version(VERSION).is_ok());
        assert!(matches!(
            check_version("0.0.0"),
            Err(SyncError::VersionMismatch {
                update: UpdateNeeded::Server,
                ..
            })
        ));
    }
}

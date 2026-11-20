//! Aides communes aux tests d'intégration (et, par `#[path]`, aux tests unitaires).
#![allow(dead_code)]

use tempfile::TempDir;

/// Dossier court sous /tmp : sous macOS, un chemin de socket Unix est limité à 104 octets.
pub fn dossier_court() -> TempDir {
    tempfile::Builder::new()
        .prefix("drc-")
        .tempdir_in("/tmp")
        .expect("dossier court sous /tmp")
}

//! Aides partagées par les tests unitaires de la lib (les tests d'intégration ont les leurs :
//! `tests/commun`).

/// Dossier court sous /tmp : sous macOS, un chemin de socket Unix est limité à 104 octets.
pub fn dossier_court() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("drc-")
        .tempdir_in("/tmp")
        .expect("dossier court sous /tmp")
}

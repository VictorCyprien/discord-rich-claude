//! Aides communes aux tests d'intégration (`mod commun;` dans chaque fichier de `tests/`).
#![allow(dead_code)]

use tempfile::TempDir;

/// Dossier court sous /tmp : sous macOS, un chemin de socket Unix est limité à 104 octets.
pub fn dossier_court() -> TempDir {
    tempfile::Builder::new()
        .prefix("drc-")
        .tempdir_in("/tmp")
        .expect("dossier court sous /tmp")
}

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use discord_rich_claude::carte::Carte;
use discord_rich_claude::chemins::COMMANDE_HOOK;
use discord_rich_claude::presence::{ClientDiscord, ErreurDiscord, Fabrique};

/// `demarrer_serveur` pose un `umask` pour tout le processus : les tests qui démarrent un
/// serveur sont sérialisés par ce verrou.
static SERIE: Mutex<()> = Mutex::new(());

pub fn serie() -> MutexGuard<'static, ()> {
    SERIE.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Clone, Debug, PartialEq)]
pub enum Entree {
    Connecter,
    Publier(Carte),
    Effacer,
    Fermer,
}

pub type Journal = Arc<Mutex<Vec<Entree>>>;

pub struct FauxClient {
    pub journal: Journal,
}

impl ClientDiscord for FauxClient {
    fn connecter(&mut self) -> Result<(), ErreurDiscord> {
        self.journal.lock().unwrap().push(Entree::Connecter);
        Ok(())
    }
    fn publier(&mut self, carte: &Carte) -> Result<(), ErreurDiscord> {
        self.journal
            .lock()
            .unwrap()
            .push(Entree::Publier(carte.clone()));
        Ok(())
    }
    fn effacer(&mut self) -> Result<(), ErreurDiscord> {
        self.journal.lock().unwrap().push(Entree::Effacer);
        Ok(())
    }
    fn fermer(&mut self) -> Result<(), ErreurDiscord> {
        self.journal.lock().unwrap().push(Entree::Fermer);
        Ok(())
    }
}

pub fn fabrique_fausse(journal: &Journal) -> Fabrique {
    let journal = Arc::clone(journal);
    Box::new(move |_| {
        Box::new(FauxClient {
            journal: Arc::clone(&journal),
        })
    })
}

pub fn cartes(journal: &Journal) -> Vec<Carte> {
    journal
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            Entree::Publier(c) => Some(c.clone()),
            _ => None,
        })
        .collect()
}

pub fn contient(journal: &Journal, entree: &Entree) -> bool {
    journal.lock().unwrap().contains(entree)
}

/// Lance la commande de hook exacte, comme Claude Code le ferait : environnement vidé sauf
/// `HOME` et `PATH`, payload JSON sur stdin.
pub fn envoyer_hook(home: &Path, payload: &serde_json::Value) {
    let mut enfant = Command::new("sh")
        .arg("-c")
        .arg(COMMANDE_HOOK)
        .env_clear()
        .env("HOME", home)
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .stdin(Stdio::piped())
        .spawn()
        .expect("lancement de sh");
    let mut stdin = enfant.stdin.take().expect("stdin");
    stdin
        .write_all(&serde_json::to_vec(payload).unwrap())
        .expect("écriture du payload");
    drop(stdin);
    let statut = enfant.wait().expect("attente de sh");
    assert!(statut.success());
}

pub fn attendre(condition: impl Fn() -> bool) {
    let fin = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < fin, "condition jamais atteinte");
        std::thread::sleep(Duration::from_millis(20));
    }
}

// --- journal de test : capture tous les logs du processus de test ---

struct Capture;
static CAPTURE: Capture = Capture;
static MESSAGES: Mutex<Vec<String>> = Mutex::new(Vec::new());

impl log::Log for Capture {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }
    fn log(&self, r: &log::Record) {
        if let Ok(mut m) = MESSAGES.lock() {
            m.push(format!("{} {} {}", r.level(), r.target(), r.args()));
        }
    }
    fn flush(&self) {}
}

pub fn installer_capture() {
    let _ = log::set_logger(&CAPTURE);
    log::set_max_level(log::LevelFilter::Debug);
}

pub fn messages() -> Vec<String> {
    MESSAGES.lock().map(|m| m.clone()).unwrap_or_default()
}

pub fn vider_messages() {
    if let Ok(mut m) = MESSAGES.lock() {
        m.clear();
    }
}

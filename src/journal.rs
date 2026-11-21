//! Journal fichier : `daemon.log` avec rotation, au format du `logging` Python, et hook de panique.

use std::cell::Cell;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use chrono::{DateTime, Local, Timelike};
use log::{LevelFilter, Log, Metadata, Record};

pub const TAILLE_MAX_JOURNAL: u64 = 1_000_000;
pub const SAUVEGARDES: u32 = 3;
const MESSAGE_PANIQUE: &str = "erreur interne (panique)";

/// Niveau Python d'un niveau `log`.
fn nom_niveau(niveau: log::Level) -> &'static str {
    match niveau {
        log::Level::Error => "ERROR",
        log::Level::Warn => "WARNING",
        log::Level::Info => "INFO",
        log::Level::Debug | log::Level::Trace => "DEBUG",
    }
}

/// `2026-11-20 10:00:00,123 INFO discord_rich_claude.daemon message`, sans fin de ligne.
pub fn formater_ligne(
    heure: &DateTime<Local>,
    niveau: log::Level,
    cible: &str,
    message: &str,
) -> String {
    format!(
        "{},{:03} {} {} {}",
        heure.format("%Y-%m-%d %H:%M:%S"),
        heure.nanosecond() / 1_000_000 % 1000,
        nom_niveau(niveau),
        cible.replace("::", "."),
        message
    )
}

/// Niveau voulu pour `DRC_LOG`, et l'avertissement à journaliser si la valeur est invalide.
/// `CRITICAL` vaut `ERROR`. Valeur absente : INFO.
pub fn niveau_depuis(valeur: Option<&str>) -> (LevelFilter, Option<String>) {
    let Some(brut) = valeur else {
        return (LevelFilter::Info, None);
    };
    let majuscules = brut.to_uppercase();
    match majuscules.as_str() {
        "DEBUG" => (LevelFilter::Debug, None),
        "INFO" => (LevelFilter::Info, None),
        "WARNING" => (LevelFilter::Warn, None),
        "ERROR" | "CRITICAL" => (LevelFilter::Error, None),
        _ => (
            LevelFilter::Info,
            Some(format!("DRC_LOG invalide ({majuscules}), repli sur INFO")),
        ),
    }
}

struct Sortie {
    base: PathBuf,
    fichier: Option<File>,
    taille: u64,
    max: u64,
    sauvegardes: u32,
}

impl Sortie {
    fn ouvrir(base: &Path, max: u64, sauvegardes: u32) -> Sortie {
        let fichier = OpenOptions::new().create(true).append(true).open(base).ok();
        let taille = fichier
            .as_ref()
            .and_then(|f| f.metadata().ok())
            .map_or(0, |m| m.len());
        Sortie {
            base: base.to_path_buf(),
            fichier,
            taille,
            max,
            sauvegardes,
        }
    }

    fn sauvegarde(&self, n: u32) -> PathBuf {
        let mut nom = self.base.clone().into_os_string();
        nom.push(format!(".{n}"));
        PathBuf::from(nom)
    }

    /// Comme `RotatingFileHandler.doRollover` : `.2` → `.3`, `.1` → `.2`, courant → `.1`.
    fn tourner(&mut self) {
        self.fichier = None;
        if self.sauvegardes > 0 {
            for i in (1..self.sauvegardes).rev() {
                let source = self.sauvegarde(i);
                if source.exists() {
                    let _ = fs::rename(&source, self.sauvegarde(i + 1));
                }
            }
            let _ = fs::rename(&self.base, self.sauvegarde(1));
        } else {
            let _ = fs::remove_file(&self.base);
        }
        self.fichier = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.base)
            .ok();
        self.taille = 0;
    }

    /// On décale avant d'écrire la ligne qui atteindrait la limite, comme Python.
    fn ecrire(&mut self, ligne: &str) {
        let octets = ligne.len() as u64 + 1;
        if self.max > 0 && self.taille > 0 && self.taille + octets >= self.max {
            self.tourner();
        }
        if let Some(f) = self.fichier.as_mut()
            && writeln!(f, "{ligne}").is_ok()
        {
            self.taille += octets;
        }
    }
}

/// Logger `log` qui écrit dans un fichier à rotation.
pub struct Journal {
    sortie: Mutex<Sortie>,
}

impl Journal {
    pub fn new(base: &Path, max: u64, sauvegardes: u32) -> Journal {
        Journal {
            sortie: Mutex::new(Sortie::ouvrir(base, max, sauvegardes)),
        }
    }
}

impl Log for Journal {
    fn enabled(&self, _: &Metadata) -> bool {
        true
    }

    fn log(&self, r: &Record) {
        let ligne = formater_ligne(&Local::now(), r.level(), r.target(), &r.args().to_string());
        let mut sortie = self.sortie.lock().unwrap_or_else(|e| e.into_inner());
        sortie.ecrire(&ligne);
    }

    fn flush(&self) {}
}

static JOURNAL: OnceLock<Journal> = OnceLock::new();

/// Installe le logger fichier. Ne panique pas si un logger est déjà installé.
pub fn configurer_logs(dossier: &Path) {
    configurer_logs_avec(dossier, std::env::var("DRC_LOG").ok().as_deref());
}

/// Comme `configurer_logs`, avec la valeur de `DRC_LOG` en paramètre.
pub fn configurer_logs_avec(dossier: &Path, drc_log: Option<&str>) {
    let _ = fs::create_dir_all(dossier);
    let journal = Journal::new(&dossier.join("daemon.log"), TAILLE_MAX_JOURNAL, SAUVEGARDES);
    let (niveau, avertissement) = niveau_depuis(drc_log);
    if log::set_logger(JOURNAL.get_or_init(|| journal)).is_err() {
        return;
    }
    log::set_max_level(niveau);
    if let Some(a) = avertissement {
        log::warn!(target: "discord_rich_claude.daemon", "{a}");
    }
}

/// Garde RAII : tant qu'elle vit, une panique sur ce thread est rattrapée et signalée
/// ailleurs (`Signaleur`, log du serveur) ; le hook se tait.
pub struct GardePanique;

thread_local! {
    static RATTRAPEES: Cell<u32> = const { Cell::new(0) };
}

pub fn panique_rattrapee() -> GardePanique {
    RATTRAPEES.with(|c| c.set(c.get() + 1));
    GardePanique
}

impl Drop for GardePanique {
    fn drop(&mut self) {
        RATTRAPEES.with(|c| c.set(c.get().saturating_sub(1)));
    }
}

/// Remplace le hook de panique par défaut : son message peut citer un `cwd`, et sous
/// launchd stderr part dans un fichier. Seul un message fixe est écrit.
pub fn installer_hook_panique() {
    std::panic::set_hook(Box::new(|_| {
        if RATTRAPEES.with(|c| c.get() > 0) {
            return;
        }
        eprintln!("discord-rich-claude : {MESSAGE_PANIQUE}");
        log::error!("{MESSAGE_PANIQUE}");
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::process::Command;

    #[test]
    fn format_de_ligne_comme_python() {
        let heure = Local
            .with_ymd_and_hms(2026, 11, 20, 10, 0, 0)
            .single()
            .unwrap()
            + chrono::Duration::milliseconds(123);
        assert_eq!(
            formater_ligne(
                &heure,
                log::Level::Info,
                "discord_rich_claude::daemon",
                "message"
            ),
            "2026-11-20 10:00:00,123 INFO discord_rich_claude.daemon message"
        );
        assert!(formater_ligne(&heure, log::Level::Warn, "a", "m").contains(" WARNING a m"));
        assert!(formater_ligne(&heure, log::Level::Error, "a", "m").contains(" ERROR a m"));
        assert!(formater_ligne(&heure, log::Level::Debug, "a", "m").contains(" DEBUG a m"));
    }

    #[test]
    fn niveau_valide_insensible_a_la_casse() {
        assert_eq!(niveau_depuis(None), (LevelFilter::Info, None));
        assert_eq!(niveau_depuis(Some("debug")), (LevelFilter::Debug, None));
        assert_eq!(niveau_depuis(Some("Warning")), (LevelFilter::Warn, None));
        assert_eq!(niveau_depuis(Some("ERROR")), (LevelFilter::Error, None));
        assert_eq!(niveau_depuis(Some("critical")), (LevelFilter::Error, None));
    }

    #[test]
    fn niveau_drc_log_invalide_replie_sur_info_avec_avertissement() {
        let (niveau, avertissement) = niveau_depuis(Some("nimporte"));
        assert_eq!(niveau, LevelFilter::Info);
        let message = avertissement.unwrap();
        assert_eq!(message, "DRC_LOG invalide (NIMPORTE), repli sur INFO");
        assert_eq!(niveau_depuis(Some("")).0, LevelFilter::Info);
        assert!(niveau_depuis(Some("")).1.is_some());
    }

    fn lire(chemin: &Path) -> String {
        fs::read_to_string(chemin).unwrap_or_default()
    }

    #[test]
    fn rotation_decale_avant_d_ecrire_et_garde_trois_sauvegardes() {
        let dossier = tempfile::tempdir().unwrap();
        let base = dossier.path().join("daemon.log");
        let mut sortie = Sortie::ouvrir(&base, 50, 3);
        // chaque ligne fait 18 octets avec la fin de ligne : la troisième atteindrait 54
        for n in 0..8 {
            sortie.ecrire(&format!("ligne-numero-{n:04}"));
        }
        assert!(lire(&base).len() < 50);
        for n in 1..=3 {
            let sauvegarde = dossier.path().join(format!("daemon.log.{n}"));
            assert!(sauvegarde.exists(), "daemon.log.{n} manque");
            assert!(lire(&sauvegarde).len() < 50);
        }
        assert!(!dossier.path().join("daemon.log.4").exists());
        // la plus récente sauvegarde est la plus proche du fichier courant
        assert_eq!(lire(&base), "ligne-numero-0006\nligne-numero-0007\n");
        assert_eq!(
            lire(&dossier.path().join("daemon.log.1")),
            "ligne-numero-0004\nligne-numero-0005\n"
        );
    }

    #[test]
    fn journal_ecrit_une_ligne_formatee() {
        let dossier = tempfile::tempdir().unwrap();
        let base = dossier.path().join("daemon.log");
        let journal = Journal::new(&base, TAILLE_MAX_JOURNAL, SAUVEGARDES);
        journal.log(
            &Record::builder()
                .level(log::Level::Warn)
                .target("discord_rich_claude::presence")
                .args(format_args!("bonjour"))
                .build(),
        );
        let texte = lire(&base);
        assert!(texte.ends_with(" WARNING discord_rich_claude.presence bonjour\n"));
        assert_eq!(texte.as_bytes()[4], b'-');
    }

    /// Enfant du test de panique : lancé par `hook_de_panique_n_ecrit_qu_un_message_fixe`.
    #[test]
    #[ignore = "lancé en sous-processus par hook_de_panique_n_ecrit_qu_un_message_fixe"]
    fn enfant_panique() {
        let Ok(dossier) = std::env::var("DRC_TEST_DOSSIER") else {
            return;
        };
        configurer_logs_avec(Path::new(&dossier), None);
        installer_hook_panique();
        let resultat = std::panic::catch_unwind(|| {
            panic!("SECRET-/Users/x/p");
        });
        assert!(resultat.is_err());
    }

    #[test]
    fn hook_de_panique_n_ecrit_qu_un_message_fixe() {
        let dossier = tempfile::tempdir().unwrap();
        let sortie = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "journal::tests::enfant_panique",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("DRC_TEST_DOSSIER", dossier.path())
            .env_remove("DRC_LOG")
            .output()
            .unwrap();
        assert!(sortie.status.success(), "l'enfant a échoué");
        let stderr = String::from_utf8_lossy(&sortie.stderr);
        let stdout = String::from_utf8_lossy(&sortie.stdout);
        for flux in [&stderr, &stdout] {
            assert!(!flux.contains("SECRET"), "fuite : {flux}");
            assert!(!flux.contains("/Users/x/p"), "fuite : {flux}");
        }
        assert!(
            stderr.contains("discord-rich-claude : erreur interne (panique)"),
            "stderr : {stderr}"
        );
        let journal = lire(&dossier.path().join("daemon.log"));
        assert!(journal.contains(" ERROR "), "journal : {journal}");
        assert!(journal.contains("erreur interne (panique)"));
        assert!(!journal.contains("SECRET"));
    }

    #[test]
    #[ignore = "lancé en sous-processus par test_drc_log_invalide_replie_sur_info_avec_avertissement"]
    fn enfant_drc_log() {
        let Ok(dossier) = std::env::var("DRC_TEST_DOSSIER") else {
            return;
        };
        configurer_logs(Path::new(&dossier));
        assert_eq!(log::max_level(), LevelFilter::Info);
        log::debug!("MARQUEUR-DEBUG");
        log::info!("MARQUEUR-INFO");
    }

    /// De bout en bout : `DRC_LOG` invalide ne fait jamais planter le démarrage (G3).
    #[test]
    fn test_drc_log_invalide_replie_sur_info_avec_avertissement() {
        let dossier = tempfile::tempdir().unwrap();
        let sortie = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "journal::tests::enfant_drc_log",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("DRC_TEST_DOSSIER", dossier.path())
            .env("DRC_LOG", "nimporte")
            .output()
            .unwrap();
        assert!(sortie.status.success(), "l'enfant a échoué");
        let journal = lire(&dossier.path().join("daemon.log"));
        let avertissements: Vec<&str> = journal
            .lines()
            .filter(|l| l.contains(" WARNING "))
            .collect();
        assert_eq!(avertissements.len(), 1, "journal : {journal}");
        assert!(avertissements[0].contains("NIMPORTE"));
        assert!(journal.contains("MARQUEUR-INFO"));
        assert!(!journal.contains("MARQUEUR-DEBUG"));
    }
}

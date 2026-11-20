//! Tests de la ligne de commande. Rien ne touche aux vrais chemins : tout passe par un
//! `Chemins::depuis_home` sous un dossier temporaire, `launchctl` et `codesign` sont factices,
//! et la confirmation est injectée.
//!
//! Tous les tests sont sérialisés par `serie()` : `demarrer` pose un `umask` global, qui
//! ferait échouer, par chance, les créations de dossiers des autres tests.

mod commun;

use std::fs;
use std::io;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use commun::{
    Journal, dossier_court, fabrique_fausse, installer_capture, messages, serie, vider_messages,
};
use discord_rich_claude::bundle::binaire_du_bundle;
use discord_rich_claude::chemins::Chemins;
use discord_rich_claude::cli::{
    confirmer_depuis, desinstaller_vers, installer_vers, lancer_vers, principal, statut_vers,
};
use discord_rich_claude::config::ConfigSurveillee;
use discord_rich_claude::daemon::{Daemon, EnMarche, Mode, demarrer};
use discord_rich_claude::installation::Sortie;
use discord_rich_claude::presence::Presence;
use discord_rich_claude::sessions::Evenement;
use discord_rich_claude::tokens::SuiviTokens;
use tempfile::TempDir;

fn chemins_temporaires() -> (TempDir, Chemins) {
    let dossier = dossier_court();
    let chemins = Chemins::depuis_home(dossier.path());
    (dossier, chemins)
}

fn daemon_factice(chemins: &Chemins) -> Arc<Daemon> {
    let journal: Journal = Default::default();
    Daemon::new(
        ConfigSurveillee::new(chemins.config.clone()),
        Presence::new(fabrique_fausse(&journal), 15.0, 15.0, Box::new(|| 0.0)),
        SuiviTokens::new(chemins.projets_claude.clone()),
        chemins.projets_claude.clone(),
        Box::new(|_| true),
        Box::new(|| 0.0),
    )
}

fn demarrer_factice(chemins: &Chemins) -> (Arc<Daemon>, EnMarche) {
    let daemon = daemon_factice(chemins);
    let en_marche = demarrer(&daemon, &chemins.socket).expect("démarrage du daemon factice");
    (daemon, en_marche)
}

/// Binaire source factice pour `install`.
fn binaire_factice(dossier: &Path) -> PathBuf {
    let binaire = dossier.join("source-discord-rich-claude");
    fs::write(&binaire, "binaire").unwrap();
    binaire
}

type Appels = Vec<String>;

fn ok() -> io::Result<Sortie> {
    Ok(Sortie {
        code: 0,
        stderr: vec![],
    })
}

fn texte(octets: Vec<u8>) -> String {
    String::from_utf8(octets).unwrap()
}

struct Resultat {
    code: i32,
    sortie: String,
    erreur: String,
    appels: Appels,
}

fn installer(chemins: &Chemins, binaire: &Path, euid: u32, confirmer: bool) -> Resultat {
    installer_avec_executeur(chemins, binaire, euid, |_| confirmer, |_| ok())
}

fn installer_avec_executeur(
    chemins: &Chemins,
    binaire: &Path,
    euid: u32,
    confirmer: impl FnOnce(&str) -> bool,
    mut faux: impl FnMut(&[&str]) -> io::Result<Sortie>,
) -> Resultat {
    let mut appels = Appels::new();
    let (mut sortie, mut erreur) = (Vec::new(), Vec::new());
    let mut executeur = |c: &[&str]| {
        appels.push(c.join(" "));
        faux(c)
    };
    let code = installer_vers(
        chemins,
        binaire,
        euid,
        &mut executeur,
        confirmer,
        &mut sortie,
        &mut erreur,
    );
    Resultat {
        code,
        sortie: texte(sortie),
        erreur: texte(erreur),
        appels,
    }
}

fn desinstaller(chemins: &Chemins, euid: u32, confirmer: impl FnOnce(&str) -> bool) -> Resultat {
    let mut appels = Appels::new();
    let (mut sortie, mut erreur) = (Vec::new(), Vec::new());
    let mut executeur = |c: &[&str]| {
        appels.push(c.join(" "));
        ok()
    };
    let code = desinstaller_vers(
        chemins,
        euid,
        &mut executeur,
        confirmer,
        &mut sortie,
        &mut erreur,
    );
    Resultat {
        code,
        sortie: texte(sortie),
        erreur: texte(erreur),
        appels,
    }
}

// --- run : G3, démarrage impossible ---

#[test]
fn test_erreur_demarrage_os_error_retourne_1_logge_et_imprime_sans_trace() {
    // Sous le LaunchAgent (KeepAlive), une erreur non rattrapée au démarrage (dossier d'état
    // invalide) donnerait une boucle d'erreurs : `lancer` logge une ligne, l'imprime sur
    // stderr sans trace, et rend 1.
    let _s = serie();
    installer_capture();
    vider_messages();
    let (dossier, chemins) = chemins_temporaires();
    let cible = dossier.path().join("ailleurs");
    fs::create_dir(&cible).unwrap();
    fs::create_dir_all(chemins.dossier_etat.parent().unwrap()).unwrap();
    symlink(&cible, &chemins.dossier_etat).unwrap(); // un lien, pas un dossier

    let appele = AtomicBool::new(false);
    let mut erreur = Vec::new();
    let code = lancer_vers(
        &chemins,
        fabrique_fausse(&Journal::default()),
        |_, _| {
            appele.store(true, Ordering::SeqCst);
            0
        },
        &mut erreur,
    );
    let erreur = texte(erreur);

    assert_eq!(code, 1);
    assert!(!appele.load(Ordering::SeqCst), "aucune UI sans démarrage");
    let erreurs: Vec<String> = messages()
        .into_iter()
        .filter(|m| m.starts_with("ERROR"))
        .collect();
    assert_eq!(erreurs.len(), 1, "{erreurs:?}");
    assert!(erreurs[0].contains("PermissionDenied"), "{erreurs:?}");
    assert!(erreurs[0].contains(&chemins.socket.display().to_string()));
    assert!(!erreur.contains("Traceback") && !erreur.contains("panicked"));
    assert!(erreur.contains("PermissionDenied"), "{erreur}");
}

#[test]
fn deuxieme_run_sort_en_1_avant_toute_ui() {
    // Point de vigilance n° 3 : un socket déjà servi => 1, message sur stderr, et la fonction
    // de menu n'est jamais appelée.
    let _s = serie();
    installer_capture();
    let (_dossier, chemins) = chemins_temporaires();
    let (_daemon, premier) = demarrer_factice(&chemins);

    let appele = AtomicBool::new(false);
    let mut erreur = Vec::new();
    let code = lancer_vers(
        &chemins,
        fabrique_fausse(&Journal::default()),
        |_, _| {
            appele.store(true, Ordering::SeqCst);
            0
        },
        &mut erreur,
    );

    assert_eq!(code, 1);
    assert!(!appele.load(Ordering::SeqCst));
    let erreur = texte(erreur);
    assert!(
        erreur.contains(&format!(
            "un daemon tourne déjà sur {}",
            chemins.socket.display()
        )),
        "{erreur}"
    );
    premier.arreter();
}

#[test]
fn run_appelle_le_menu_apres_le_demarrage() {
    let _s = serie();
    let (_dossier, chemins) = chemins_temporaires();
    let socket = chemins.socket.clone();
    let code = lancer_vers(
        &chemins,
        fabrique_fausse(&Journal::default()),
        |_, en_marche| {
            assert!(socket.exists(), "le socket est lié avant l'interface");
            en_marche.arreter();
            7
        },
        &mut Vec::new(),
    );
    assert_eq!(code, 7);
}

// --- status ---

#[test]
fn test_status_sans_daemon() {
    let _s = serie();
    let (_dossier, chemins) = chemins_temporaires();
    let mut sortie = Vec::new();
    assert_eq!(statut_vers(&chemins, &mut sortie), 1);
    assert!(texte(sortie).contains("injoignable"));
}

#[test]
fn test_status_avec_daemon() {
    let _s = serie();
    let (_dossier, chemins) = chemins_temporaires();
    let (daemon, en_marche) = demarrer_factice(&chemins);
    for id in ["s1", "s2"] {
        daemon.recevoir(Evenement {
            nom: "SessionStart".into(),
            session_id: id.into(),
            recu_a: 0.0,
            cwd: None,
            tool_name: None,
            agent_id: None,
            transcript_path: None,
            pid: None,
        });
    }
    let mut sortie = Vec::new();
    assert_eq!(statut_vers(&chemins, &mut sortie), 0);
    let sortie = texte(sortie);
    assert!(sortie.contains("daemon : actif"), "{sortie}");
    assert!(sortie.contains("discord : non connecté"), "{sortie}");
    assert!(sortie.contains("sessions : 2"), "{sortie}");
    en_marche.arreter();
}

#[test]
fn status_affiche_l_etat() {
    let _s = serie();
    let (_dossier, chemins) = chemins_temporaires();
    let (daemon, en_marche) = demarrer_factice(&chemins);
    daemon.changer_mode(Mode::Pause);
    let mut sortie = Vec::new();
    assert_eq!(statut_vers(&chemins, &mut sortie), 0);
    assert!(texte(sortie).contains("état : En pause"));
    en_marche.arreter();
}

#[test]
fn test_commande_inconnue() {
    let _s = serie();
    assert_eq!(principal(vec!["inconnue".into()]), 2);
    assert_eq!(principal(vec![]), 2);
}

// --- install, uninstall ---

#[test]
fn test_install_settings_invalide_imprime_sur_stderr_sans_trace_et_retourne_1() {
    let _s = serie();
    // Modification concurrente détectée à la relecture : annoncée sur stderr, sans trace, et
    // rien d'autre n'est tenté.
    let (dossier, chemins) = chemins_temporaires();
    fs::create_dir_all(chemins.settings_claude.parent().unwrap()).unwrap();
    fs::write(&chemins.settings_claude, "{}").unwrap();
    let settings = chemins.settings_claude.clone();
    let r = installer_avec_executeur(
        &chemins,
        &binaire_factice(dossier.path()),
        501,
        |_| {
            fs::write(&settings, r#"{"model": "opus"}"#).unwrap();
            true
        },
        |_| ok(),
    );
    assert_eq!(r.code, 1);
    assert!(!r.erreur.contains("Traceback"));
    assert!(r.erreur.contains("relance la commande"), "{}", r.erreur);
    assert!(r.erreur.contains("non modifié"));
    assert!(r.appels.is_empty());
    assert!(!chemins.config.exists() && !chemins.launch_agent.exists());
}

#[test]
fn test_install_settings_json_invalide() {
    let _s = serie();
    let (dossier, chemins) = chemins_temporaires();
    fs::create_dir_all(chemins.settings_claude.parent().unwrap()).unwrap();
    fs::write(&chemins.settings_claude, "pas du json").unwrap();
    let r = installer(&chemins, &binaire_factice(dossier.path()), 501, true);
    assert_eq!(r.code, 1);
    assert!(r.erreur.contains("non modifié"));
    assert!(r.appels.is_empty());
}

#[test]
fn test_uninstall_settings_invalide_imprime_sur_stderr_sans_trace_et_retourne_1() {
    let _s = serie();
    let (_dossier, chemins) = chemins_temporaires();
    fs::create_dir_all(chemins.settings_claude.parent().unwrap()).unwrap();
    fs::write(&chemins.settings_claude, r#"{"hooks": 3}"#).unwrap();
    let r = desinstaller(&chemins, 501, |_| true);
    assert_eq!(r.code, 1);
    assert!(!r.erreur.contains("Traceback"));
    assert!(r.erreur.contains("hooks"), "{}", r.erreur);
    assert!(r.erreur.contains("non modifié"));
    assert!(r.appels.is_empty());
}

#[test]
fn test_install_refuse_sudo() {
    let _s = serie();
    let (dossier, chemins) = chemins_temporaires();
    let r = installer(&chemins, &binaire_factice(dossier.path()), 0, true);
    assert_eq!(r.code, 1);
    assert!(r.erreur.contains("sudo"));
    assert!(!r.erreur.contains("Traceback"));
    assert!(r.sortie.is_empty());
    assert!(r.appels.is_empty());
    assert!(!chemins.settings_claude.exists() && !chemins.config.exists());
    assert!(!chemins.applications.exists() && !chemins.launch_agent.exists());
}

#[test]
fn test_uninstall_refuse_sudo() {
    let _s = serie();
    let (_dossier, chemins) = chemins_temporaires();
    let r = desinstaller(&chemins, 0, |_| true);
    assert_eq!(r.code, 1);
    assert!(r.erreur.contains("sudo"));
    assert!(!r.erreur.contains("Traceback"));
    assert!(r.appels.is_empty());
}

#[test]
fn test_install_sans_executable_absolu_refuse_et_n_ecrit_rien() {
    let _s = serie();
    // La source du binaire est introuvable : rien pour le LaunchAgent, pas de launchctl.
    let (dossier, chemins) = chemins_temporaires();
    let r = installer(&chemins, &dossier.path().join("absent"), 501, true);
    assert_eq!(r.code, 1);
    assert!(!r.erreur.contains("Traceback"));
    assert!(!chemins.launch_agent.exists());
    assert!(
        r.appels.iter().all(|a| !a.starts_with("launchctl")),
        "{:?}",
        r.appels
    );
}

#[test]
fn test_install_imprime_l_executable_choisi() {
    let _s = serie();
    let (dossier, chemins) = chemins_temporaires();
    let r = installer(&chemins, &binaire_factice(dossier.path()), 501, true);
    assert_eq!(r.code, 0, "{}", r.erreur);
    let attendu = binaire_du_bundle(&chemins.applications);
    assert!(
        r.sortie
            .contains(&format!("exécutable : {}", attendu.display())),
        "{}",
        r.sortie
    );
    for etape in [
        "1/4 Hooks Claude Code",
        "2/4 Configuration",
        "3/4 Application",
        "4/4 LaunchAgent",
        "LaunchAgent chargé",
    ] {
        assert!(r.sortie.contains(etape), "{etape} absent de {}", r.sortie);
    }
    assert!(r.appels.iter().any(|a| a.starts_with("codesign")));
    assert!(
        r.appels
            .iter()
            .any(|a| a.starts_with("launchctl bootstrap"))
    );
}

#[test]
fn install_plist_pointe_dans_le_bundle() {
    let _s = serie();
    // Point de vigilance n° 4 : ProgramArguments[0] est le binaire du bundle.
    let (dossier, chemins) = chemins_temporaires();
    let r = installer(&chemins, &binaire_factice(dossier.path()), 501, true);
    assert_eq!(r.code, 0, "{}", r.erreur);
    let plist = fs::read_to_string(&chemins.launch_agent).unwrap();
    let binaire = binaire_du_bundle(&chemins.applications);
    assert!(
        plist.contains(&format!(
            "<key>ProgramArguments</key>\n\t<array>\n\t\t<string>{}</string>\n\t\t<string>run</string>",
            binaire.display()
        )),
        "{plist}"
    );
    assert!(binaire.is_file());
}

#[test]
fn test_install_refuse_par_l_utilisateur_n_ecrit_rien() {
    let _s = serie();
    let (dossier, chemins) = chemins_temporaires();
    let r = installer(&chemins, &binaire_factice(dossier.path()), 501, false);
    assert_eq!(r.code, 1);
    assert!(r.sortie.contains("installation annulée"));
    assert!(!chemins.settings_claude.exists());
    assert!(!chemins.applications.exists() && !chemins.launch_agent.exists());
    assert!(r.appels.is_empty());
}

#[test]
fn test_confirmer_eof_est_un_refus() {
    let _s = serie();
    let mut sortie = Vec::new();
    let mut entree = io::Cursor::new(Vec::new());
    assert!(!confirmer_depuis(
        &mut entree,
        &mut sortie,
        Path::new("/x/settings.json"),
        "diff"
    ));
    let sortie = texte(sortie);
    assert!(
        sortie.contains("diff") && sortie.contains("[o/N]"),
        "{sortie}"
    );
    assert!(sortie.contains("/x/settings.json"));
}

/// Lecteur qui échoue : Python y voyait un `KeyboardInterrupt`. En Rust, Ctrl-C tue le
/// processus avant toute écriture ; une erreur de lecture reste un refus.
struct LecteurInterrompu;
impl io::Read for LecteurInterrompu {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("interrompu"))
    }
}

#[test]
fn test_confirmer_ctrl_c_est_un_refus() {
    let _s = serie();
    let mut entree = io::BufReader::new(LecteurInterrompu);
    assert!(!confirmer_depuis(
        &mut entree,
        &mut Vec::new(),
        Path::new("/x/settings.json"),
        "diff"
    ));
}

#[test]
fn confirmer_n_accepte_que_o() {
    let _s = serie();
    for (reponse, attendu) in [
        ("o\n", true),
        ("O\n", true),
        (" o \n", true),
        ("oui\n", false),
        ("n\n", false),
        ("\n", false),
    ] {
        let mut entree = io::Cursor::new(reponse.as_bytes().to_vec());
        assert_eq!(
            confirmer_depuis(&mut entree, &mut Vec::new(), Path::new("/s"), "d"),
            attendu,
            "{reponse:?}"
        );
    }
}

#[test]
fn test_install_config_echoue_stderr_sans_trace_et_retourne_1() {
    let _s = serie();
    let (dossier, mut chemins) = chemins_temporaires();
    // Le parent de la config est un fichier : sa création échoue.
    let fichier = dossier.path().join("fichier");
    fs::write(&fichier, "").unwrap();
    chemins.config = fichier.join("config.toml");
    let r = installer(&chemins, &binaire_factice(dossier.path()), 501, true);
    assert_eq!(r.code, 1);
    assert!(!r.erreur.contains("Traceback"));
    assert!(r.erreur.contains("AlreadyExists"), "{}", r.erreur);
    assert!(
        r.appels.is_empty(),
        "l'étape suivante ne doit pas être tentée"
    );
    assert!(!chemins.launch_agent.exists());
}

#[test]
fn test_install_ecriture_plist_echoue_stderr_sans_trace_et_retourne_1() {
    let _s = serie();
    let (dossier, mut chemins) = chemins_temporaires();
    let fichier = dossier.path().join("fichier");
    fs::write(&fichier, "").unwrap();
    chemins.launch_agent = fichier.join("agent.plist");
    let r = installer(&chemins, &binaire_factice(dossier.path()), 501, true);
    assert_eq!(r.code, 1);
    assert!(!r.erreur.contains("Traceback"));
    assert!(r.erreur.contains("AlreadyExists"), "{}", r.erreur);
    assert!(r.appels.iter().all(|a| !a.starts_with("launchctl")));
}

#[test]
fn test_install_launchctl_echoue_message_contient_stderr() {
    let _s = serie();
    let (dossier, chemins) = chemins_temporaires();
    let r = installer_avec_executeur(
        &chemins,
        &binaire_factice(dossier.path()),
        501,
        |_| true,
        |c| {
            if c.get(1) == Some(&"bootstrap") {
                Ok(Sortie {
                    code: 1,
                    stderr: b"boom".to_vec(),
                })
            } else {
                ok()
            }
        },
    );
    assert_eq!(r.code, 1);
    assert!(!r.erreur.contains("Traceback"));
    assert!(r.erreur.contains("boom"), "{}", r.erreur);
    assert!(r.erreur.contains("launchctl a échoué (1)"));
}

#[test]
fn uninstall_retire_hooks_agent_et_bundle_mais_garde_la_config() {
    let _s = serie();
    let (dossier, chemins) = chemins_temporaires();
    let binaire = binaire_factice(dossier.path());
    assert_eq!(installer(&chemins, &binaire, 501, true).code, 0);
    assert!(chemins.config.exists() && chemins.launch_agent.exists());
    let r = desinstaller(&chemins, 501, |_| true);
    assert_eq!(r.code, 0, "{}", r.erreur);
    assert!(r.sortie.contains(&format!(
        "Désinstallé. Config conservée : {}",
        chemins.config.display()
    )));
    assert!(chemins.config.exists());
    assert!(!chemins.launch_agent.exists());
    assert!(
        !chemins
            .applications
            .join("Discord Rich Claude.app")
            .exists()
    );
    assert!(r.appels.iter().any(|a| a.starts_with("launchctl bootout")));
    let settings = fs::read_to_string(&chemins.settings_claude).unwrap();
    assert!(!settings.contains("discord-rich-claude"));
}

#[test]
fn uninstall_refuse_par_l_utilisateur_ne_touche_a_rien() {
    let _s = serie();
    let (dossier, chemins) = chemins_temporaires();
    assert_eq!(
        installer(&chemins, &binaire_factice(dossier.path()), 501, true).code,
        0
    );
    let r = desinstaller(&chemins, 501, |_| false);
    assert_eq!(r.code, 1);
    assert!(r.sortie.contains("désinstallation annulée"));
    assert!(chemins.launch_agent.exists());
    assert!(r.appels.is_empty());
}

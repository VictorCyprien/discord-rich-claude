//! Tests de bout en bout : vrai socket, vrai `curl` (par la commande de hook), faux Discord.
//!
//! Ils vivent dans leur propre processus et sont sérialisés par `serie()` : `demarrer_serveur`
//! pose un `umask` global au processus.

mod commun;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Local;
use commun::{
    Entree, Journal, attendre, cartes, contient, dossier_court, envoyer_hook, fabrique_fausse,
    installer_capture, messages, serie, vider_messages,
};
use discord_rich_claude::config::ConfigSurveillee;
use discord_rich_claude::daemon::{Daemon, Mode, demarrer, horloge_systeme};
use discord_rich_claude::presence::Presence;
use discord_rich_claude::sessions::Evenement;
use discord_rich_claude::tokens::SuiviTokens;
use serde_json::json;

type Horloge = Arc<Mutex<f64>>;

struct Banc {
    home: tempfile::TempDir,
    _tmp: tempfile::TempDir,
    socket: PathBuf,
    projet: PathBuf,
    projets_claude: PathBuf,
    config: PathBuf,
    journal: Journal,
    horloge: Horloge,
    pids: Arc<Mutex<Vec<i32>>>,
    daemon: Arc<Daemon>,
}

/// `reel` : horloge murale réelle ; sinon une horloge contrôlable (partagée avec `Presence`,
/// intervalle minimal de 15 s).
fn banc(contenu_config: impl Fn(&Path) -> String, reel: bool) -> Banc {
    let home = dossier_court();
    let tmp = tempfile::tempdir().unwrap();
    let socket = home
        .path()
        .join(".local/state/discord-rich-claude/daemon.sock");
    let projets_claude = tmp.path().join("projects");
    fs::create_dir_all(&projets_claude).unwrap();
    let projet = tmp.path().join("MonProjet");
    fs::create_dir_all(&projet).unwrap();
    let config = tmp.path().join("config.toml");
    fs::write(&config, contenu_config(&projet)).unwrap();
    let journal: Journal = Arc::default();
    let horloge: Horloge = Arc::new(Mutex::new(1_000_000.0));
    let pids: Arc<Mutex<Vec<i32>>> = Arc::default();
    let (h1, h2) = (Arc::clone(&horloge), Arc::clone(&horloge));
    let pids_fil = Arc::clone(&pids);
    let (presence, horloge_daemon): (_, Box<dyn Fn() -> f64 + Send + Sync>) = if reel {
        (
            Presence::new(
                fabrique_fausse(&journal),
                0.0,
                0.0,
                discord_rich_claude::presence::horloge_monotone(),
            ),
            horloge_systeme(),
        )
    } else {
        (
            Presence::new(
                fabrique_fausse(&journal),
                15.0,
                15.0,
                Box::new(move || *h1.lock().unwrap()),
            ),
            Box::new(move || *h2.lock().unwrap()),
        )
    };
    let daemon = Daemon::new(
        ConfigSurveillee::new(config.clone()),
        presence,
        SuiviTokens::new(projets_claude.clone()),
        projets_claude.clone(),
        Box::new(move |pid| {
            pids_fil.lock().unwrap().push(pid);
            true
        }),
        horloge_daemon,
    );
    Banc {
        home,
        _tmp: tmp,
        socket,
        projet,
        projets_claude,
        config,
        journal,
        horloge,
        pids,
        daemon,
    }
}

fn config_ordinaire(projet: &Path) -> String {
    format!(
        "client_id = \"42\"\n[projets]\nautorises = [\"{}\"]\n",
        projet.display()
    )
}

fn hook(b: &Banc, payload: serde_json::Value) {
    envoyer_hook(b.home.path(), &payload);
}

fn session_start(b: &Banc, transcript: Option<&Path>) {
    let mut payload = json!({
        "hook_event_name": "SessionStart",
        "session_id": "s1",
        "cwd": b.projet.to_str().unwrap(),
    });
    if let Some(t) = transcript {
        payload["transcript_path"] = json!(t.to_str().unwrap());
    }
    hook(b, payload);
}

fn nb_sessions(b: &Banc) -> u64 {
    b.daemon.etat()["sessions"].as_u64().unwrap()
}

fn ecrire_transcript(chemin: &Path) {
    fs::create_dir_all(chemin.parent().unwrap()).unwrap();
    let ligne = json!({
        "type": "assistant",
        "requestId": "r1",
        "timestamp": Local::now().to_rfc3339(),
        "message": {
            "id": "m1",
            "model": "claude-opus-5",
            "usage": {
                "input_tokens": 10,
                "output_tokens": 5,
                "cache_creation_input_tokens": 100,
                "cache_read_input_tokens": 9999,
            },
        },
    });
    fs::write(chemin, format!("{ligne}\n")).unwrap();
}

#[test]
fn test_bout_en_bout() {
    let _s = serie();
    let b = banc(config_ordinaire, true);
    let transcript = b.projets_claude.join("p/s1.jsonl");
    ecrire_transcript(&transcript);
    let en_marche = demarrer(&b.daemon, &b.socket).unwrap();

    session_start(&b, Some(&transcript));
    hook(
        &b,
        json!({
            "hook_event_name": "PreToolUse",
            "session_id": "s1",
            "tool_name": "Edit",
            "tool_input": {"content": "SECRET-CONTENU"},
        }),
    );
    b.daemon.controler();

    // $PPID du shell de hook = ce processus
    assert!(
        b.pids
            .lock()
            .unwrap()
            .contains(&(std::process::id() as i32))
    );
    assert!(!b.daemon.debug_sessions().contains("SECRET"));
    assert_eq!(nb_sessions(&b), 1);

    attendre(|| {
        cartes(&b.journal).iter().any(|c| {
            c.details == "Écrit du code · MonProjet"
                && (c.state == "Opus 5 · 115 tokens" || c.state == "1 session · 115 aujourd'hui")
        })
    });
    assert!(b.daemon.etat()["discord"].as_bool().unwrap());

    hook(
        &b,
        json!({"hook_event_name": "SessionEnd", "session_id": "s1"}),
    );
    attendre(|| contient(&b.journal, &Entree::Effacer));

    en_marche.arreter();
    assert!(!b.socket.exists());
    assert!(contient(&b.journal, &Entree::Fermer));
}

#[test]
fn test_bout_en_bout_rien_de_prive_ne_fuit() {
    let _s = serie();
    installer_capture();
    vider_messages();
    let marqueur = "SECRET-MARQUEUR-9F3C";
    let nom_prive = "ProjetClientConfidentielXYZ";
    let b = banc(
        |_| "client_id = \"42\"\n[projets]\nautorises = []\n".into(),
        true,
    );
    let dossier_prive = b._tmp.path().join(nom_prive);
    fs::create_dir_all(&dossier_prive).unwrap();
    let en_marche = demarrer(&b.daemon, &b.socket).unwrap();

    hook(
        &b,
        json!({
            "hook_event_name": "UserPromptSubmit",
            "session_id": "s1",
            "cwd": dossier_prive.to_str().unwrap(),
            "prompt": marqueur,
        }),
    );
    hook(
        &b,
        json!({
            "hook_event_name": "PreToolUse",
            "session_id": "s1",
            "tool_name": "Edit",
            "tool_input": {"content": marqueur},
        }),
    );
    hook(
        &b,
        json!({
            "hook_event_name": "Stop",
            "session_id": "s1",
            "last_assistant_message": marqueur,
        }),
    );
    b.daemon.controler();
    attendre(|| !cartes(&b.journal).is_empty());
    en_marche.arreter();

    for m in messages() {
        assert!(!m.contains(marqueur), "fuite dans le log : {m}");
        assert!(!m.contains(nom_prive), "fuite dans le log : {m}");
    }
    let publiees = cartes(&b.journal);
    assert!(!publiees.is_empty());
    for c in publiees {
        assert!(!c.details.contains(marqueur) && !c.state.contains(marqueur));
        assert!(!c.details.contains(nom_prive) && !c.state.contains(nom_prive));
        assert!(c.details.ends_with("Projet privé"));
    }
}

#[test]
fn test_client_id_absent_logge_une_erreur_au_demarrage_et_apres_rechargement() {
    let _s = serie();
    installer_capture();
    vider_messages();
    let b = banc(|_| String::new(), false);
    let erreurs = || {
        messages()
            .iter()
            .filter(|m| m.starts_with("ERROR ") && m.contains("client_id absent"))
            .count()
    };
    for _ in 0..3 {
        b.daemon.controler();
    }
    assert_eq!(erreurs(), 1);

    fs::write(&b.config, "inactif_apres_min = 5\n").unwrap();
    // la date de modification doit changer d'au moins un instant
    let f = fs::OpenOptions::new().write(true).open(&b.config).unwrap();
    f.set_modified(std::time::SystemTime::now() + Duration::from_secs(5))
        .unwrap();
    b.daemon.controler();
    assert_eq!(erreurs(), 2);
}

#[test]
fn test_carte_effacee_apres_masquage_puis_revient_avec_son_debut_dorigine() {
    let _s = serie();
    let b = banc(
        |_| "client_id = \"42\"\nmasquer_apres_min = 1\n".into(),
        false,
    );
    let t0 = 1_000_000.0;
    b.daemon.recevoir(Evenement {
        nom: "SessionStart".into(),
        session_id: "s1".into(),
        recu_a: t0,
        cwd: None,
        tool_name: None,
        agent_id: None,
        transcript_path: None,
        pid: None,
    });
    assert!(b.daemon.carte().is_some());
    b.daemon.publier();
    assert!(contient(&b.journal, &Entree::Connecter));

    *b.horloge.lock().unwrap() = t0 + 61.0; // masquer_apres_min dépassé
    assert!(b.daemon.carte().is_none());
    b.daemon.publier();
    assert!(contient(&b.journal, &Entree::Effacer));

    *b.horloge.lock().unwrap() = t0 + 62.0;
    b.daemon.recevoir(Evenement {
        nom: "PreToolUse".into(),
        session_id: "s1".into(),
        recu_a: t0 + 62.0,
        cwd: None,
        tool_name: Some("Edit".into()),
        agent_id: None,
        transcript_path: None,
        pid: None,
    });
    let carte = b.daemon.carte().unwrap();
    assert_eq!(carte.debut, t0 as i64); // début d'origine de la session
}

#[test]
fn arreter_ignore_les_hooks_et_ferme_discord() {
    let _s = serie();
    let b = banc(config_ordinaire, true);
    let en_marche = demarrer(&b.daemon, &b.socket).unwrap();
    session_start(&b, None);
    attendre(|| !cartes(&b.journal).is_empty());

    b.daemon.changer_mode(Mode::Arrete);
    assert!(contient(&b.journal, &Entree::Effacer));
    assert!(contient(&b.journal, &Entree::Fermer));
    assert_eq!(nb_sessions(&b), 0);

    hook(
        &b,
        json!({"hook_event_name": "UserPromptSubmit", "session_id": "s2"}),
    );
    assert_eq!(nb_sessions(&b), 0);
    assert_eq!(b.daemon.etat()["etat"], "arrete");
    assert_eq!(b.daemon.mode(), Mode::Arrete);
    let avant = b.journal.lock().unwrap().len();
    std::thread::sleep(Duration::from_millis(1300)); // un tour de la boucle carte
    assert_eq!(b.journal.lock().unwrap().len(), avant);
    en_marche.arreter();
}

#[test]
fn pause_puis_allumer_republie_la_carte_courante() {
    let _s = serie();
    let b = banc(config_ordinaire, false);
    let en_marche = demarrer(&b.daemon, &b.socket).unwrap();
    session_start(&b, None);
    attendre(|| !cartes(&b.journal).is_empty());
    let avant = cartes(&b.journal).len();

    b.daemon.changer_mode(Mode::Pause);
    assert!(contient(&b.journal, &Entree::Effacer));
    assert!(!contient(&b.journal, &Entree::Fermer));
    assert_eq!(b.daemon.etat()["etat"], "pause");

    // les hooks continuent de mettre à jour les sessions
    hook(
        &b,
        json!({"hook_event_name": "UserPromptSubmit", "session_id": "s2",
               "cwd": b.projet.to_str().unwrap()}),
    );
    assert_eq!(nb_sessions(&b), 2);

    // en pause, rien n'est publié même après 15 s
    *b.horloge.lock().unwrap() += 20.0;
    std::thread::sleep(Duration::from_millis(1300));
    assert_eq!(cartes(&b.journal).len(), avant);

    b.daemon.changer_mode(Mode::Allume);
    *b.horloge.lock().unwrap() += 15.0;
    attendre(|| cartes(&b.journal).len() > avant);
    let derniere = cartes(&b.journal).pop().unwrap();
    assert_eq!(Some(derniere), b.daemon.carte());
    en_marche.arreter();
}

#[test]
fn allumer_apres_arreter_reconstruit_depuis_les_hooks() {
    let _s = serie();
    let b = banc(config_ordinaire, true);
    let en_marche = demarrer(&b.daemon, &b.socket).unwrap();
    session_start(&b, None);
    attendre(|| nb_sessions(&b) == 1);
    b.daemon.changer_mode(Mode::Arrete);
    assert_eq!(nb_sessions(&b), 0);

    b.daemon.changer_mode(Mode::Allume);
    assert_eq!(b.daemon.etat()["etat"], "allume");
    hook(
        &b,
        json!({"hook_event_name": "UserPromptSubmit", "session_id": "s1",
               "cwd": b.projet.to_str().unwrap()}),
    );
    assert_eq!(nb_sessions(&b), 1);
    attendre(|| {
        cartes(&b.journal)
            .last()
            .is_some_and(|c| c.details == "Réfléchit · MonProjet")
    });
    en_marche.arreter();
}

#[test]
fn l_arret_est_rapide_et_supprime_le_socket() {
    let _s = serie();
    installer_capture();
    vider_messages();
    let b = banc(config_ordinaire, true);
    let en_marche = demarrer(&b.daemon, &b.socket).unwrap();
    assert!(b.socket.exists());
    let debut = Instant::now();
    en_marche.arreter();
    assert!(debut.elapsed() < Duration::from_secs(1));
    assert!(!b.socket.exists());
    let m = messages();
    assert!(m.iter().any(|l| l.contains("daemon à l'écoute sur")));
    assert!(m.iter().any(|l| l.ends_with("daemon arrêté")));
}

#[test]
fn configurer_logs_ne_panique_pas_si_un_logger_existe() {
    let _s = serie();
    installer_capture();
    let dossier = tempfile::tempdir().unwrap();
    discord_rich_claude::journal::configurer_logs_avec(dossier.path(), Some("DEBUG"));
}

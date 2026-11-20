//! Tests du serveur sur un vrai socket Unix (curl-like : HTTP brut écrit sur le socket).
//!
//! Ils vivent dans leur propre processus : `demarrer_serveur` pose un `umask` le temps du
//! `bind`, ce qui est global au processus et ferait échouer, par chance, des tests qui créent
//! des dossiers au même instant. Ici tout est donc sérialisé par `serie()`.

mod commun;

use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::{FileTypeExt, PermissionsExt, symlink};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use commun::dossier_court;
use discord_rich_claude::serveur::{
    ErreurDemarrage, Gestionnaire, Serveur, demarrer_serveur, interroger_etat,
};
use discord_rich_claude::sessions::Evenement;
use serde_json::json;

static SERIE: Mutex<()> = Mutex::new(());

/// Sérialise les tests du fichier (voir l'en-tête).
fn serie() -> MutexGuard<'static, ()> {
    SERIE.lock().unwrap_or_else(|e| e.into_inner())
}

fn corps(valeur: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&valeur).unwrap()
}

// --- journal de test : capture tous les logs du processus de test ---

struct Journal;
static JOURNAL: Journal = Journal;
static MESSAGES: Mutex<Vec<String>> = Mutex::new(Vec::new());

impl log::Log for Journal {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }
    fn log(&self, r: &log::Record) {
        if let Ok(mut m) = MESSAGES.lock() {
            m.push(format!("{} {}", r.level(), r.args()));
        }
    }
    fn flush(&self) {}
}

fn installer_journal() {
    let _ = log::set_logger(&JOURNAL);
    log::set_max_level(log::LevelFilter::Debug);
}

fn messages() -> Vec<String> {
    MESSAGES.lock().map(|m| m.clone()).unwrap_or_default()
}

// --- serveur ---

#[derive(Default)]
struct Faux {
    recus: Mutex<Vec<Evenement>>,
    appels: AtomicUsize,
    echec_premier: bool,
    panique_evenement: bool,
    panique_etat: bool,
}

impl Gestionnaire for Faux {
    fn sur_evenement(&self, ev: Evenement) -> Result<(), &'static str> {
        let n = self.appels.fetch_add(1, Ordering::SeqCst);
        if self.panique_evenement {
            panic!("SECRET-PANIQUE-EVENEMENT /Users/victor/secret/fichier.py");
        }
        self.recus.lock().unwrap().push(ev);
        if self.echec_premier && n == 0 {
            return Err("boum");
        }
        Ok(())
    }
    fn etat(&self) -> serde_json::Value {
        if self.panique_etat {
            panic!("SECRET-PANIQUE-ETAT");
        }
        json!({"sessions": 2})
    }
}

struct Banc {
    dossier: tempfile::TempDir,
    socket: PathBuf,
    faux: Arc<Faux>,
    serveur: Option<Serveur>,
}

fn banc_avec(faux: Faux) -> Banc {
    installer_journal();
    let dossier = dossier_court();
    let socket = dossier.path().join("etat/daemon.sock");
    let faux = Arc::new(faux);
    let serveur = demarrer_serveur(&socket, faux.clone(), dossier.path().join("projets")).unwrap();
    Banc {
        dossier,
        socket,
        faux,
        serveur: Some(serveur),
    }
}

fn banc() -> Banc {
    banc_avec(Faux::default())
}

impl Banc {
    fn requete(&self, brut: &[u8]) -> Vec<u8> {
        requete(&self.socket, brut)
    }
    fn recus(&self) -> Vec<Evenement> {
        self.faux.recus.lock().unwrap().clone()
    }
}

impl Drop for Banc {
    fn drop(&mut self) {
        if let Some(s) = self.serveur.take() {
            s.arreter();
        }
        let _ = &self.dossier;
    }
}

fn requete(socket: &Path, brut: &[u8]) -> Vec<u8> {
    let mut flux = UnixStream::connect(socket).unwrap();
    flux.set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    flux.write_all(brut).unwrap();
    let mut reponse = Vec::new();
    let _ = flux.read_to_end(&mut reponse);
    reponse
}

fn post(corps: &[u8], entetes: &str) -> Vec<u8> {
    let mut v = format!(
        "POST /event HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n{entetes}\r\n",
        corps.len()
    )
    .into_bytes();
    v.extend_from_slice(corps);
    v
}

fn evenement_stop() -> Vec<u8> {
    corps(json!({"hook_event_name": "Stop", "session_id": "s1"}))
}

#[test]
fn post_event() {
    let _serie = serie();
    let b = banc();
    let r = b.requete(&post(&evenement_stop(), "X-Claude-Pid: 77\r\n"));
    assert!(r.starts_with(b"HTTP/1.1 204"));
    let recus = b.recus();
    assert_eq!(
        recus
            .iter()
            .map(|e| (e.nom.as_str(), e.pid))
            .collect::<Vec<_>>(),
        vec![("Stop", Some(77))]
    );
}

#[test]
fn reponses_a_l_octet_pres() {
    let _serie = serie();
    let b = banc();
    assert_eq!(
        b.requete(&post(&evenement_stop(), "")),
        b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    assert_eq!(
        b.requete(b"GET / HTTP/1.1\r\n\r\n"),
        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    assert_eq!(
        b.requete(b"GET /etat HTTP/1.1\r\n\r\n"),
        b"HTTP/1.1 200 OK\r\nContent-Length: 14\r\nConnection: close\r\nContent-Type: application/json\r\n\r\n{\"sessions\":2}"
    );
    assert_eq!(
        b.requete(b"POST /event HTTP/1.1\r\n\r\n"),
        b"HTTP/1.1 411 Length Required\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
}

#[test]
fn expect_100_continue() {
    let _serie = serie();
    let b = banc();
    let r = b.requete(&post(&evenement_stop(), "Expect: 100-continue\r\n"));
    assert!(r.starts_with(b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 204"));
    assert_eq!(b.recus().len(), 1);
}

#[test]
fn corps_trop_gros_refuse() {
    let _serie = serie();
    let b = banc();
    let r = b.requete(b"POST /event HTTP/1.1\r\nContent-Length: 17000000\r\n\r\n");
    assert!(r.starts_with(b"HTTP/1.1 413"));
    assert!(b.recus().is_empty());
}

#[test]
fn content_length_negatif_refuse() {
    let _serie = serie();
    let b = banc();
    let r = b.requete(b"POST /event HTTP/1.1\r\nContent-Length: -1\r\n\r\n");
    assert!(r.starts_with(b"HTTP/1.1 413"));
}

#[test]
fn sans_content_length() {
    let _serie = serie();
    let b = banc();
    assert!(
        b.requete(b"POST /event HTTP/1.1\r\n\r\n")
            .starts_with(b"HTTP/1.1 411")
    );
}

#[test]
fn route_inconnue() {
    let _serie = serie();
    let b = banc();
    assert!(
        b.requete(b"GET / HTTP/1.1\r\n\r\n")
            .starts_with(b"HTTP/1.1 404")
    );
    assert!(
        b.requete(b"POST /etat HTTP/1.1\r\n\r\n")
            .starts_with(b"HTTP/1.1 404")
    );
    assert!(
        b.requete(b"GET /event HTTP/1.1\r\n\r\n")
            .starts_with(b"HTTP/1.1 404")
    );
}

#[test]
fn etat() {
    let _serie = serie();
    let b = banc();
    assert_eq!(
        interroger_etat(&b.socket, Duration::from_secs(1)),
        Some(json!({"sessions": 2}))
    );
}

#[test]
fn etat_sans_daemon() {
    let _serie = serie();
    let d = dossier_court();
    assert_eq!(
        interroger_etat(&d.path().join("absent.sock"), Duration::from_secs(1)),
        None
    );
}

#[test]
fn interroger_etat_refuse_un_non_200_et_un_json_invalide() {
    let _serie = serie();
    let d = dossier_court();
    let socket = d.path().join("faux.sock");
    let ecouteur = UnixListener::bind(&socket).unwrap();
    let fil = thread::spawn(move || {
        for reponse in [
            &b"HTTP/1.1 404 Not Found\r\n\r\n{}"[..],
            &b"HTTP/1.1 200 OK\r\n\r\npas du json"[..],
            &b"HTTP/1.1 200 OK\r\n\r\n{\"a\":1}"[..],
        ] {
            let (mut flux, _) = ecouteur.accept().unwrap();
            let mut tampon = [0u8; 256];
            let _ = flux.read(&mut tampon);
            flux.write_all(reponse).unwrap();
        }
    });
    assert_eq!(interroger_etat(&socket, Duration::from_secs(2)), None);
    assert_eq!(interroger_etat(&socket, Duration::from_secs(2)), None);
    assert_eq!(
        interroger_etat(&socket, Duration::from_secs(2)),
        Some(json!({"a": 1}))
    );
    fil.join().unwrap();
}

#[test]
fn permissions() {
    let _serie = serie();
    let b = banc();
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&b.socket), 0o600);
    assert_eq!(mode(b.socket.parent().unwrap()), 0o700);
}

#[test]
fn deuxieme_instance_refusee() {
    let _serie = serie();
    let b = banc();
    let r = demarrer_serveur(&b.socket, Arc::new(Faux::default()), PathBuf::from("/x"));
    assert!(matches!(r, Err(ErreurDemarrage::DejaLance(_))));
    // la première instance répond toujours
    assert!(interroger_etat(&b.socket, Duration::from_secs(1)).is_some());
}

#[test]
fn socket_orphelin_remplace() {
    let _serie = serie();
    let d = dossier_court();
    let socket = d.path().join("orphelin.sock");
    fs::write(&socket, b"").unwrap();
    let s = demarrer_serveur(&socket, Arc::new(Faux::default()), PathBuf::from("/x")).unwrap();
    s.arreter();
}

#[test]
fn vrai_socket_mort_remplace() {
    let _serie = serie();
    let d = dossier_court();
    let socket = d.path().join("mort.sock");
    drop(UnixListener::bind(&socket).unwrap()); // le fichier reste, plus personne n'écoute
    assert!(
        fs::symlink_metadata(&socket)
            .unwrap()
            .file_type()
            .is_socket()
    );
    let s = demarrer_serveur(&socket, Arc::new(Faux::default()), PathBuf::from("/x")).unwrap();
    assert!(interroger_etat(&socket, Duration::from_secs(1)).is_some());
    s.arreter();
}

#[test]
fn symlink_au_chemin_socket_est_remplace() {
    let _serie = serie();
    let d = dossier_court();
    let cible = d.path().join("ailleurs"); // jamais créée : le lien est pendant
    let dossier = d.path().join("etat");
    fs::create_dir_all(&dossier).unwrap();
    let socket = dossier.join("daemon.sock");
    symlink(&cible, &socket).unwrap();
    let s = demarrer_serveur(&socket, Arc::new(Faux::default()), PathBuf::from("/x")).unwrap();
    assert!(
        fs::symlink_metadata(&socket)
            .unwrap()
            .file_type()
            .is_socket()
    );
    assert!(fs::symlink_metadata(&cible).is_err());
    s.arreter();
}

#[test]
fn dossier_etat_symlink_refuse() {
    let _serie = serie();
    let d = dossier_court();
    let ailleurs = d.path().join("ailleurs");
    fs::create_dir(&ailleurs).unwrap();
    let lien = d.path().join("etat");
    symlink(&ailleurs, &lien).unwrap();
    let r = demarrer_serveur(
        &lien.join("daemon.sock"),
        Arc::new(Faux::default()),
        PathBuf::from("/x"),
    );
    match r {
        Err(ErreurDemarrage::Io(e)) => assert_eq!(e.kind(), io::ErrorKind::PermissionDenied),
        autre => panic!("attendu PermissionDenied, obtenu {:?}", autre.err()),
    }
    assert!(!ailleurs.join("daemon.sock").exists());
}

#[test]
fn dossier_etat_fichier_refuse() {
    let _serie = serie();
    let d = dossier_court();
    let fichier = d.path().join("etat");
    fs::write(&fichier, b"").unwrap();
    let r = demarrer_serveur(
        &fichier.join("daemon.sock"),
        Arc::new(Faux::default()),
        PathBuf::from("/x"),
    );
    assert!(matches!(r, Err(ErreurDemarrage::Io(_))));
}

#[test]
fn erreur_du_gestionnaire_n_arrete_pas_le_serveur() {
    let _serie = serie();
    let b = banc_avec(Faux {
        echec_premier: true,
        ..Faux::default()
    });
    let brute = post(&evenement_stop(), "");
    assert!(b.requete(&brute).starts_with(b"HTTP/1.1 500"));
    assert!(b.requete(&brute).starts_with(b"HTTP/1.1 204"));
    assert!(
        messages()
            .iter()
            .any(|m| m == "ERROR erreur sur un événement Stop : boum")
    );
}

#[test]
fn erreur_inattendue_repond_500() {
    let _serie = serie();
    let b = banc_avec(Faux {
        panique_etat: true,
        ..Faux::default()
    });
    let r = b.requete(b"GET /etat HTTP/1.1\r\nHost: localhost\r\n\r\n");
    assert!(r.starts_with(b"HTTP/1.1 500"));
    // le serveur répond toujours après coup
    assert!(
        b.requete(&post(&evenement_stop(), ""))
            .starts_with(b"HTTP/1.1 204")
    );
    assert!(
        messages()
            .iter()
            .any(|m| m == "ERROR erreur inattendue dans le traitement d'une requête : panique")
    );
}

#[test]
fn gestionnaire_qui_panique_repond_500_sans_fuite() {
    let _serie = serie();
    let b = banc_avec(Faux {
        panique_evenement: true,
        ..Faux::default()
    });
    let secret = corps(json!({
        "hook_event_name": "Stop", "session_id": "s1",
        "prompt": "SECRET-PROMPT-LOG",
        "tool_input": {"file_path": "/Users/victor/secret/fichier.py"},
    }));
    assert!(
        b.requete(&post(&secret, "X-Secret: SECRET-ENTETE\r\n"))
            .starts_with(b"HTTP/1.1 500")
    );
    assert!(b.requete(&post(&secret, "")).starts_with(b"HTTP/1.1 500"));
    assert_pas_de_fuite();
}

fn assert_pas_de_fuite() {
    for m in messages() {
        for interdit in [
            "SECRET",
            "/Users/victor/secret",
            "fichier.py",
            "Stop\",",
            "tool_input",
        ] {
            assert!(!m.contains(interdit), "fuite {interdit:?} dans {m:?}");
        }
    }
}

#[test]
fn erreur_du_gestionnaire_ne_fuite_pas_dans_les_logs() {
    let _serie = serie();
    let b = banc_avec(Faux {
        echec_premier: true,
        ..Faux::default()
    });
    let secret = corps(json!({
        "hook_event_name": "Stop", "session_id": "s1",
        "prompt": "SECRET-PROMPT-LOG",
        "tool_input": {"file_path": "/Users/victor/secret/fichier.py"},
    }));
    assert!(
        b.requete(&post(&secret, "X-Secret: SECRET-ENTETE\r\n"))
            .starts_with(b"HTTP/1.1 500")
    );
    assert_pas_de_fuite();
}

#[test]
fn requetes_mal_formees_sans_reponse_et_sans_fuite() {
    let _serie = serie();
    let b = banc();
    // ligne de requête sans trois morceaux
    assert!(b.requete(b"GET /etat\r\n\r\n").is_empty());
    // Content-Length non entier
    assert!(
        b.requete(b"POST /event HTTP/1.1\r\nContent-Length: SECRET-LONGUEUR\r\n\r\n")
            .is_empty()
    );
    // connexion coupée avant la fin du corps
    assert!(
        b.requete(b"POST /event HTTP/1.1\r\nContent-Length: 50\r\n\r\n{\"SECRET\"")
            .is_empty()
    );
    // en-tête sans fin
    let mut long = b"GET / HTTP/1.1\r\nX: ".to_vec();
    long.extend(vec![b'a'; 70_000]);
    assert!(b.requete(&long).is_empty());
    assert!(
        messages()
            .iter()
            .any(|m| m.starts_with("DEBUG requête rejetée : "))
    );
    assert_pas_de_fuite();
    assert!(b.recus().is_empty());
}

#[test]
fn echeance_totale_de_l_entete() {
    let _serie = serie();
    let b = banc();
    let debut = Instant::now();
    let mut flux = UnixStream::connect(&b.socket).unwrap();
    flux.set_read_timeout(Some(Duration::from_millis(50)))
        .unwrap();
    // un octet toutes les 400 ms : chaque lecture arrive bien avant 2 s, mais le total non
    let envoi = b"GET /etat HTTP/1.1\r\nX: aaaaaaaaaaaaaaaaaaaaaaaaaaaaa\r\n";
    let mut ferme = false;
    let mut recu = Vec::new();
    for octet in envoi {
        if flux.write_all(&[*octet]).is_err() {
            ferme = true;
            break;
        }
        let mut tampon = [0u8; 64];
        match flux.read(&mut tampon) {
            Ok(0) => {
                ferme = true;
                break;
            }
            Ok(n) => recu.extend_from_slice(&tampon[..n]),
            Err(_) => {}
        }
        thread::sleep(Duration::from_millis(400));
        if debut.elapsed() > Duration::from_secs(8) {
            break;
        }
    }
    let ecoule = debut.elapsed();
    assert!(ferme, "la connexion aurait dû être fermée");
    assert!(ecoule < Duration::from_millis(3500), "{ecoule:?}");
    assert!(ecoule >= Duration::from_millis(1900), "{ecoule:?}");
    assert!(recu.is_empty(), "aucune réponse attendue");
}

#[test]
fn echeance_totale_du_corps() {
    let _serie = serie();
    let b = banc();
    let debut = Instant::now();
    let mut flux = UnixStream::connect(&b.socket).unwrap();
    flux.set_read_timeout(Some(Duration::from_millis(50)))
        .unwrap();
    flux.write_all(b"POST /event HTTP/1.1\r\nContent-Length: 1000\r\n\r\n")
        .unwrap();
    let mut ferme = false;
    for _ in 0..30 {
        if flux.write_all(b"x").is_err() {
            ferme = true;
            break;
        }
        let mut tampon = [0u8; 64];
        if matches!(flux.read(&mut tampon), Ok(0)) {
            ferme = true;
            break;
        }
        thread::sleep(Duration::from_millis(400));
    }
    let ecoule = debut.elapsed();
    assert!(ferme);
    assert!(ecoule < Duration::from_millis(3500), "{ecoule:?}");
    assert!(b.recus().is_empty());
}

#[test]
fn arreter_ferme_l_ecoute_et_laisse_le_socket() {
    let _serie = serie();
    let mut b = banc();
    let socket = b.socket.clone();
    b.serveur.take().unwrap().arreter();
    assert!(UnixStream::connect(&socket).is_err());
    assert!(fs::symlink_metadata(&socket).is_ok());
    // on peut repartir sur le même chemin
    let s = demarrer_serveur(&socket, Arc::new(Faux::default()), PathBuf::from("/x")).unwrap();
    assert!(interroger_etat(&socket, Duration::from_secs(1)).is_some());
    s.arreter();
}

#[test]
fn connexions_concurrentes() {
    let _serie = serie();
    let b = banc();
    let socket = b.socket.clone();
    let fils: Vec<_> = (0..16)
        .map(|_| {
            let socket = socket.clone();
            thread::spawn(move || requete(&socket, &post(&evenement_stop(), "")))
        })
        .collect();
    for f in fils {
        assert!(f.join().unwrap().starts_with(b"HTTP/1.1 204"));
    }
    assert_eq!(b.recus().len(), 16);
}

#[test]
fn un_client_lent_ne_bloque_pas_les_autres() {
    let _serie = serie();
    let b = banc();
    let mut lent = UnixStream::connect(&b.socket).unwrap();
    lent.write_all(b"GET /et").unwrap();
    assert!(
        b.requete(&post(&evenement_stop(), ""))
            .starts_with(b"HTTP/1.1 204")
    );
}

#[test]
fn vrai_curl_comme_le_hook() {
    let _serie = serie();
    let b = banc();
    // la commande du hook : curl lit l'événement sur stdin et l'envoie au socket
    let mut enfant = match std::process::Command::new("curl")
        .args(["-s", "--max-time", "5", "--unix-socket"])
        .arg(&b.socket)
        .args(["-H", "X-Claude-Pid: 4321", "--data-binary", "@-"])
        .args(["-w", "%{http_code}", "http://localhost/event"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
    {
        Ok(e) => e,
        Err(_) => return, // pas de curl sur cette machine
    };
    enfant
        .stdin
        .take()
        .unwrap()
        .write_all(&evenement_stop())
        .unwrap();
    let sortie = enfant.wait_with_output().unwrap();
    assert_eq!(String::from_utf8_lossy(&sortie.stdout), "204");
    let recus = b.recus();
    assert_eq!(recus.len(), 1);
    assert_eq!(recus[0].pid, Some(4321));
}

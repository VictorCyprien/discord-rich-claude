//! Connexion à Discord : publication à débit limité, reconnexion, pause, et
//! client IPC écrit à la main. Voir la spec, sections 5, 9 et 14.

use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::carte::Carte;

/// Discord documente « one update per 15 seconds » ; au-delà, la présence peut
/// être vidée au lieu d'être mise en file (discord-api-docs#668).
pub const INTERVALLE_MIN_S: f64 = 15.0;
pub const REESSAI_S: f64 = 15.0;
/// Borne de lecture et d'écriture sur le socket : un pair qui accepte la
/// connexion sans jamais répondre ne doit pas bloquer la boucle carte à 1 s.
pub const DELAI_POIGNEE_MAIN_S: u64 = 5;
/// Longueur maximale d'une trame acceptée en réponse (64 Kio).
const TAILLE_MAX_TRAME: u32 = 64 * 1024;

const OP_POIGNEE_DE_MAIN: u32 = 0;
const OP_TRAME: u32 = 1;
const OP_FERMETURE: u32 = 2;

/// Erreur de Discord. Seul le genre est conservé, jamais le contenu d'une
/// réponse ni le `Display` d'une erreur système : il peut partir dans un log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ErreurDiscord {
    pub genre: &'static str,
}

const AUCUN_SOCKET: ErreurDiscord = ErreurDiscord {
    genre: "aucun socket Discord",
};
const DELAI_DEPASSE: ErreurDiscord = ErreurDiscord {
    genre: "délai dépassé",
};
const CONNEXION_FERMEE: ErreurDiscord = ErreurDiscord {
    genre: "connexion fermée",
};
const REPONSE_INVALIDE: ErreurDiscord = ErreurDiscord {
    genre: "réponse invalide",
};
const REFUSE: ErreurDiscord = ErreurDiscord {
    genre: "refusé par Discord",
};

pub trait ClientDiscord: Send {
    fn connecter(&mut self) -> Result<(), ErreurDiscord>;
    fn publier(&mut self, carte: &Carte) -> Result<(), ErreurDiscord>;
    fn effacer(&mut self) -> Result<(), ErreurDiscord>;
    fn fermer(&mut self) -> Result<(), ErreurDiscord>;
}

pub type Fabrique = Box<dyn Fn(&str) -> Box<dyn ClientDiscord> + Send>;

/// Horloge monotone en secondes, comptées depuis le premier appel de cette fabrique.
pub fn horloge_monotone() -> Box<dyn Fn() -> f64 + Send> {
    let origine = Instant::now();
    Box::new(move || origine.elapsed().as_secs_f64())
}

pub struct Presence {
    fabrique: Fabrique,
    intervalle_min_s: f64,
    reessai_s: f64,
    horloge: Box<dyn Fn() -> f64 + Send>,
    client: Option<Box<dyn ClientDiscord>>,
    client_id: Option<String>,
    derniere: Option<Carte>,
    dernier_envoi: f64,
    dernier_essai: f64,
    /// Genre de l'erreur en cours (`None` si le dernier essai a réussi) : pour
    /// ne pas noyer le journal d'un essai toutes les 15 s tant que Discord est
    /// injoignable.
    genre_erreur_en_cours: Option<&'static str>,
}

impl Presence {
    pub fn new(
        fabrique: Fabrique,
        intervalle_min_s: f64,
        reessai_s: f64,
        horloge: Box<dyn Fn() -> f64 + Send>,
    ) -> Self {
        Self {
            fabrique,
            intervalle_min_s,
            reessai_s,
            horloge,
            client: None,
            client_id: None,
            derniere: None,
            dernier_envoi: f64::NEG_INFINITY,
            dernier_essai: f64::NEG_INFINITY,
            genre_erreur_en_cours: None,
        }
    }

    pub fn connecte(&self) -> bool {
        self.client.is_some()
    }

    pub fn soumettre(&mut self, carte: Option<&Carte>, client_id: Option<&str>) {
        if carte.is_none() || client_id.is_none() || client_id != self.client_id.as_deref() {
            self.fermer();
        }
        let (Some(carte), Some(client_id)) = (carte, client_id) else {
            return;
        };
        let maintenant = (self.horloge)();
        if self.client.is_none() {
            if maintenant - self.dernier_essai < self.reessai_s {
                return;
            }
            self.dernier_essai = maintenant;
            let mut client = (self.fabrique)(client_id);
            if let Err(e) = client.connecter() {
                self.signaler_echec("Discord injoignable, nouvel essai dans", e);
                return;
            }
            self.client = Some(client);
            self.client_id = Some(client_id.to_string());
            self.genre_erreur_en_cours = None;
            log::info!("connecté à Discord");
        }
        if self.derniere.as_ref() == Some(carte)
            || maintenant - self.dernier_envoi < self.intervalle_min_s
        {
            return;
        }
        let Some(client) = self.client.as_mut() else {
            return;
        };
        if let Err(e) = client.publier(carte) {
            self.abandonner("publication refusée, reconnexion dans", e, maintenant);
            return;
        }
        self.derniere = Some(carte.clone());
        self.dernier_envoi = maintenant;
    }

    /// Pause : efface la carte affichée en gardant la connexion. La reprise
    /// attend l'intervalle minimal, comme après une publication.
    pub fn effacer_carte(&mut self) {
        let Some(client) = self.client.as_mut() else {
            return;
        };
        let maintenant = (self.horloge)();
        match client.effacer() {
            Ok(()) => {
                self.derniere = None;
                self.dernier_envoi = maintenant;
            }
            Err(e) => self.abandonner("publication refusée, reconnexion dans", e, maintenant),
        }
    }

    /// Jette le client après un échec : un nouvel essai aura lieu dans `reessai_s`.
    fn abandonner(&mut self, gabarit: &str, e: ErreurDiscord, maintenant: f64) {
        self.signaler_echec(gabarit, e);
        let client = self.client.take();
        self.client_id = None;
        self.derniere = None;
        self.dernier_essai = maintenant;
        if let Some(mut client) = client {
            let _ = client.fermer();
        }
    }

    /// Tant que Discord reste injoignable, un essai toutes les 15 s ne doit
    /// produire qu'une seule ligne WARNING par genre d'erreur ; les répétitions
    /// du même genre partent en DEBUG. Seul le genre est loggé.
    fn signaler_echec(&mut self, gabarit: &str, e: ErreurDiscord) {
        if self.genre_erreur_en_cours == Some(e.genre) {
            log::debug!("{gabarit} {:.0} s : {}", self.reessai_s, e.genre);
        } else {
            log::warn!("{gabarit} {:.0} s : {}", self.reessai_s, e.genre);
            self.genre_erreur_en_cours = Some(e.genre);
        }
    }

    pub fn fermer(&mut self) {
        let client = self.client.take();
        self.client_id = None;
        self.derniere = None;
        self.dernier_envoi = f64::NEG_INFINITY;
        let Some(mut client) = client else {
            return;
        };
        if let Err(e) = client.effacer() {
            log::warn!("fermeture de la connexion Discord : {}", e.genre);
        }
        if let Err(e) = client.fermer() {
            log::warn!("fermeture de la connexion Discord : {}", e.genre);
        }
    }
}

/// Client IPC Discord écrit à la main (spec 14.1). Après une `Err` (hors
/// `fermer`), il est inutilisable : `Presence` le jette et en recrée un.
pub struct ClientIpc {
    client_id: String,
    chemins: Vec<PathBuf>,
    flux: Option<UnixStream>,
    nonce: u64,
}

fn erreur_io(e: &io::Error) -> ErreurDiscord {
    match e.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => DELAI_DEPASSE,
        _ => CONNEXION_FERMEE,
    }
}

impl ClientIpc {
    pub fn new(client_id: &str, chemins: Vec<PathBuf>) -> Self {
        Self {
            client_id: client_id.to_string(),
            chemins,
            flux: None,
            nonce: 0,
        }
    }

    fn ecrire(&mut self, op: u32, corps: &Value) -> Result<(), ErreurDiscord> {
        let flux = self.flux.as_mut().ok_or(CONNEXION_FERMEE)?;
        let charge = serde_json::to_vec(corps).map_err(|_| REPONSE_INVALIDE)?;
        let mut trame = Vec::with_capacity(8 + charge.len());
        trame.extend(op.to_le_bytes());
        trame.extend((charge.len() as u32).to_le_bytes());
        trame.extend(charge);
        flux.write_all(&trame).map_err(|e| erreur_io(&e))
    }

    fn lire(&mut self) -> Result<(u32, Value), ErreurDiscord> {
        let flux = self.flux.as_mut().ok_or(CONNEXION_FERMEE)?;
        let mut entete = [0u8; 8];
        flux.read_exact(&mut entete).map_err(|e| erreur_io(&e))?;
        let op = u32::from_le_bytes([entete[0], entete[1], entete[2], entete[3]]);
        let longueur = u32::from_le_bytes([entete[4], entete[5], entete[6], entete[7]]);
        if longueur > TAILLE_MAX_TRAME {
            return Err(REPONSE_INVALIDE);
        }
        let mut corps = vec![0u8; longueur as usize];
        flux.read_exact(&mut corps).map_err(|e| erreur_io(&e))?;
        let json = serde_json::from_slice(&corps).map_err(|_| REPONSE_INVALIDE)?;
        Ok((op, json))
    }

    fn poignee_de_main(&mut self) -> Result<(), ErreurDiscord> {
        let corps = json!({"v": 1, "client_id": self.client_id});
        self.ecrire(OP_POIGNEE_DE_MAIN, &corps)?;
        match self.lire()? {
            (OP_TRAME, r) if r["evt"] == "READY" => Ok(()),
            (OP_TRAME, r) if r["evt"] == "ERROR" => Err(REFUSE),
            (OP_FERMETURE, _) => Err(REFUSE),
            _ => Err(REPONSE_INVALIDE),
        }
    }

    fn set_activity(&mut self, args: Value) -> Result<(), ErreurDiscord> {
        self.nonce += 1;
        let corps = json!({"cmd": "SET_ACTIVITY", "args": args, "nonce": self.nonce.to_string()});
        self.ecrire(OP_TRAME, &corps)?;
        match self.lire()? {
            (OP_TRAME, r) if r["evt"] == "ERROR" => Err(REFUSE),
            (OP_TRAME, _) => Ok(()),
            (OP_FERMETURE, _) => Err(REFUSE),
            _ => Err(REPONSE_INVALIDE),
        }
    }
}

impl ClientDiscord for ClientIpc {
    fn connecter(&mut self) -> Result<(), ErreurDiscord> {
        let delai = Some(Duration::from_secs(DELAI_POIGNEE_MAIN_S));
        let flux = self
            .chemins
            .iter()
            .find_map(|chemin| UnixStream::connect(chemin).ok())
            .ok_or(AUCUN_SOCKET)?;
        flux.set_read_timeout(delai).map_err(|e| erreur_io(&e))?;
        flux.set_write_timeout(delai).map_err(|e| erreur_io(&e))?;
        self.flux = Some(flux);
        let resultat = self.poignee_de_main();
        if resultat.is_err() {
            self.flux = None;
        }
        resultat
    }

    fn publier(&mut self, carte: &Carte) -> Result<(), ErreurDiscord> {
        let activite = json!({
            "details": carte.details,
            "state": carte.state,
            "timestamps": {"start": carte.debut},
            "assets": {"large_image": carte.large_image, "large_text": carte.large_text},
        });
        self.set_activity(json!({"pid": std::process::id(), "activity": activite}))
    }

    fn effacer(&mut self) -> Result<(), ErreurDiscord> {
        self.set_activity(json!({"pid": std::process::id()}))
    }

    fn fermer(&mut self) -> Result<(), ErreurDiscord> {
        // Jamais connecté (ou déjà fermé) : rien à fermer, pas d'erreur.
        if self.flux.is_none() {
            return Ok(());
        }
        // La fermeture est complète dans tous les cas : le socket est libéré même si
        // l'envoi de la trame échoue. On rend la première erreur rencontrée.
        let corps = json!({"v": 1, "client_id": self.client_id});
        let envoi = self.ecrire(OP_FERMETURE, &corps);
        let arret = match self.flux.take() {
            Some(flux) => match flux.shutdown(std::net::Shutdown::Both) {
                // Le pair est déjà parti : rien de plus à fermer.
                Err(e) if e.kind() != io::ErrorKind::NotConnected => Err(erreur_io(&e)),
                _ => Ok(()),
            },
            None => Ok(()),
        };
        envoi.and(arret)
    }
}

/// Dossiers candidats dans l'ordre de pypresence, puis `discord-ipc-0` à `-9`.
pub fn chemins_ipc_depuis(lire: impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let base = ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"]
        .iter()
        .find_map(|nom| lire(nom).filter(|v| !v.is_empty()))
        .unwrap_or_else(|| "/tmp".to_string());
    (0..10)
        .map(|i| PathBuf::from(&base).join(format!("discord-ipc-{i}")))
        .collect()
}

pub fn chemins_ipc() -> Vec<PathBuf> {
    chemins_ipc_depuis(|nom| std::env::var(nom).ok())
}

pub fn fabrique_sur(chemins: Vec<PathBuf>) -> Fabrique {
    Box::new(move |client_id| Box::new(ClientIpc::new(client_id, chemins.clone())))
}

pub fn fabrique_reelle() -> Fabrique {
    fabrique_sur(chemins_ipc())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::os::unix::net::UnixListener;
    use std::sync::{Arc, Mutex, Once};
    use std::thread::JoinHandle;

    fn dossier_court() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("drc-")
            .tempdir_in("/tmp")
            .expect("dossier court sous /tmp")
    }

    fn carte(details: &str) -> Carte {
        Carte {
            details: details.to_string(),
            state: "Opus 5 · 1 k tokens".to_string(),
            debut: 0,
            large_image: "logo",
            large_text: "Claude Code",
        }
    }

    // ---- Journal des logs, par thread de test ----

    thread_local! {
        static LOGS: RefCell<Vec<(log::Level, String)>> = const { RefCell::new(Vec::new()) };
    }
    struct Capture;
    impl log::Log for Capture {
        fn enabled(&self, _: &log::Metadata) -> bool {
            true
        }
        fn log(&self, r: &log::Record) {
            LOGS.with(|l| l.borrow_mut().push((r.level(), r.args().to_string())));
        }
        fn flush(&self) {}
    }
    static CAPTURE: Capture = Capture;
    fn init_logs() {
        static UNE_FOIS: Once = Once::new();
        UNE_FOIS.call_once(|| {
            let _ = log::set_logger(&CAPTURE);
            log::set_max_level(log::LevelFilter::Debug);
        });
        LOGS.with(|l| l.borrow_mut().clear());
    }

    // ---- Faux client ----

    type Journal = Arc<Mutex<Vec<String>>>;

    #[derive(Clone, Default)]
    struct Options {
        echec_connexion: bool,
        echec_publication: bool,
        echec_effacement: bool,
        echec_fermeture: bool,
        erreurs_connexion: Arc<Mutex<Vec<&'static str>>>,
    }

    struct FauxClient {
        journal: Journal,
        client_id: String,
        options: Options,
    }

    impl ClientDiscord for FauxClient {
        fn connecter(&mut self) -> Result<(), ErreurDiscord> {
            self.journal
                .lock()
                .unwrap()
                .push(format!("connecter:{}", self.client_id));
            let mut erreurs = self.options.erreurs_connexion.lock().unwrap();
            if !erreurs.is_empty() {
                return Err(ErreurDiscord {
                    genre: erreurs.remove(0),
                });
            }
            if self.options.echec_connexion {
                return Err(AUCUN_SOCKET);
            }
            Ok(())
        }
        fn publier(&mut self, carte: &Carte) -> Result<(), ErreurDiscord> {
            if self.options.echec_publication {
                self.journal.lock().unwrap().push("publier-echec".into());
                return Err(CONNEXION_FERMEE);
            }
            self.journal
                .lock()
                .unwrap()
                .push(format!("publier:{}", carte.details));
            Ok(())
        }
        fn effacer(&mut self) -> Result<(), ErreurDiscord> {
            self.journal.lock().unwrap().push("effacer".into());
            if self.options.echec_effacement {
                return Err(CONNEXION_FERMEE);
            }
            Ok(())
        }
        fn fermer(&mut self) -> Result<(), ErreurDiscord> {
            self.journal.lock().unwrap().push("fermer".into());
            if self.options.echec_fermeture {
                return Err(CONNEXION_FERMEE);
            }
            Ok(())
        }
    }

    #[derive(Clone, Default)]
    struct Horloge(Arc<Mutex<f64>>);
    impl Horloge {
        fn regler(&self, t: f64) {
            *self.0.lock().unwrap() = t;
        }
    }

    fn presence_avec(
        journal: &Journal,
        horloge: &Horloge,
        options: Options,
        reessai: f64,
    ) -> Presence {
        let journal = journal.clone();
        let h = horloge.clone();
        Presence::new(
            Box::new(move |id| {
                Box::new(FauxClient {
                    journal: journal.clone(),
                    client_id: id.to_string(),
                    options: options.clone(),
                })
            }),
            15.0,
            reessai,
            Box::new(move || *h.0.lock().unwrap()),
        )
    }

    fn presence(journal: &Journal, horloge: &Horloge) -> Presence {
        presence_avec(journal, horloge, Options::default(), 15.0)
    }

    fn contenu(journal: &Journal) -> Vec<String> {
        journal.lock().unwrap().clone()
    }

    fn compte(journal: &Journal, ligne: &str) -> usize {
        contenu(journal).iter().filter(|l| *l == ligne).count()
    }

    #[test]
    fn test_premiere_carte_connecte_et_publie() {
        let (journal, h) = (Journal::default(), Horloge::default());
        let mut p = presence(&journal, &h);
        p.soumettre(Some(&carte("A")), Some("42"));
        assert_eq!(contenu(&journal), ["connecter:42", "publier:A"]);
        assert!(p.connecte());
    }

    #[test]
    fn test_meme_carte_pas_republiee() {
        let (journal, h) = (Journal::default(), Horloge::default());
        let mut p = presence(&journal, &h);
        p.soumettre(Some(&carte("A")), Some("42"));
        h.regler(100.0);
        p.soumettre(Some(&carte("A")), Some("42"));
        assert_eq!(compte(&journal, "publier:A"), 1);
    }

    #[test]
    fn test_rafale_regroupee_la_derniere_gagne() {
        let (journal, h) = (Journal::default(), Horloge::default());
        let mut p = presence(&journal, &h);
        p.soumettre(Some(&carte("A")), Some("42"));
        h.regler(5.0);
        p.soumettre(Some(&carte("B")), Some("42"));
        h.regler(10.0);
        p.soumettre(Some(&carte("C")), Some("42"));
        assert_eq!(compte(&journal, "publier:B"), 0);
        h.regler(15.0);
        p.soumettre(Some(&carte("C")), Some("42"));
        assert_eq!(contenu(&journal).last().unwrap(), "publier:C");
        assert_eq!(compte(&journal, "publier:B"), 0);
    }

    #[test]
    fn test_plus_de_carte_efface_et_ferme_puis_reconnecte() {
        let (journal, h) = (Journal::default(), Horloge::default());
        let mut p = presence(&journal, &h);
        p.soumettre(Some(&carte("A")), Some("42"));
        p.soumettre(None, Some("42"));
        let j = contenu(&journal);
        assert_eq!(j[j.len() - 2..], ["effacer", "fermer"]);
        assert!(!p.connecte());
        p.soumettre(None, Some("42"));
        assert_eq!(compte(&journal, "fermer"), 1);
        h.regler(20.0);
        p.soumettre(Some(&carte("A")), Some("42"));
        let j = contenu(&journal);
        assert_eq!(j[j.len() - 2..], ["connecter:42", "publier:A"]);
    }

    #[test]
    fn test_sans_client_id_rien_ne_se_connecte() {
        let (journal, h) = (Journal::default(), Horloge::default());
        let mut p = presence(&journal, &h);
        p.soumettre(Some(&carte("A")), None);
        assert!(contenu(&journal).is_empty());
    }

    #[test]
    fn test_connexion_refusee_reessai_apres_15_s() {
        let (journal, h) = (Journal::default(), Horloge::default());
        let options = Options {
            echec_connexion: true,
            ..Options::default()
        };
        let mut p = presence_avec(&journal, &h, options, 15.0);
        p.soumettre(Some(&carte("A")), Some("42"));
        h.regler(10.0);
        p.soumettre(Some(&carte("A")), Some("42"));
        assert_eq!(contenu(&journal), ["connecter:42"]);
        h.regler(15.0);
        p.soumettre(Some(&carte("A")), Some("42"));
        assert_eq!(contenu(&journal), ["connecter:42", "connecter:42"]);
    }

    #[test]
    fn test_publication_refusee_abandonne_puis_reessaie() {
        let (journal, h) = (Journal::default(), Horloge::default());
        let options = Options {
            echec_publication: true,
            ..Options::default()
        };
        let mut p = presence_avec(&journal, &h, options, 15.0);
        p.soumettre(Some(&carte("A")), Some("42"));
        let j = contenu(&journal);
        assert_eq!(j[j.len() - 2..], ["publier-echec", "fermer"]);
        assert!(!p.connecte());
        h.regler(10.0);
        p.soumettre(Some(&carte("A")), Some("42"));
        assert_eq!(compte(&journal, "connecter:42"), 1);
        h.regler(15.0);
        p.soumettre(Some(&carte("A")), Some("42"));
        assert_eq!(compte(&journal, "connecter:42"), 2);
    }

    #[test]
    fn test_changement_de_client_id_reconnecte() {
        let (journal, h) = (Journal::default(), Horloge::default());
        let mut p = presence(&journal, &h);
        p.soumettre(Some(&carte("A")), Some("42"));
        h.regler(20.0);
        p.soumettre(Some(&carte("A")), Some("43"));
        let j = contenu(&journal);
        assert_eq!(
            j[j.len() - 4..],
            ["effacer", "fermer", "connecter:43", "publier:A"]
        );
    }

    #[test]
    fn test_fermeture_echouee_loggee_en_warning() {
        // Une étape de fermeture qui échoue est loggée en WARNING, jamais silencieuse, et
        // `fermer` ne relève rien.
        init_logs();
        let (journal, h) = (Journal::default(), Horloge::default());
        let options = Options {
            echec_fermeture: true,
            ..Options::default()
        };
        let mut p = presence_avec(&journal, &h, options, 0.0);
        p.soumettre(Some(&carte("A")), Some("42"));
        LOGS.with(|l| l.borrow_mut().clear());
        p.fermer();
        let logs = LOGS.with(|l| l.borrow().clone());
        let avertissements: Vec<&String> = logs
            .iter()
            .filter(|(n, _)| *n == log::Level::Warn)
            .map(|(_, m)| m)
            .collect();
        assert_eq!(avertissements.len(), 1, "{logs:?}");
        assert!(
            avertissements[0].contains(CONNEXION_FERMEE.genre),
            "{avertissements:?}"
        );
    }

    #[test]
    fn test_echecs_de_connexion_repetes_un_seul_warning_puis_debug() {
        init_logs();
        let (journal, h) = (Journal::default(), Horloge::default());
        let options = Options {
            erreurs_connexion: Arc::new(Mutex::new(vec![
                AUCUN_SOCKET.genre,
                AUCUN_SOCKET.genre,
                AUCUN_SOCKET.genre,
                AUCUN_SOCKET.genre,
                AUCUN_SOCKET.genre,
                DELAI_DEPASSE.genre,
            ])),
            ..Options::default()
        };
        let mut p = presence_avec(&journal, &h, options, 0.0);
        for i in 0..7 {
            h.regler(f64::from(i));
            p.soumettre(Some(&carte("A")), Some("42"));
        }
        let logs = LOGS.with(|l| l.borrow().clone());
        let niveau = |n: log::Level| -> Vec<&String> {
            logs.iter()
                .filter(|(l, _)| *l == n)
                .map(|(_, m)| m)
                .collect()
        };
        let warnings = niveau(log::Level::Warn);
        assert_eq!(warnings.len(), 2);
        assert!(warnings[0].contains("aucun socket Discord"));
        assert!(warnings[1].contains("délai dépassé"));
        assert_eq!(niveau(log::Level::Debug).len(), 4);
        for (_, m) in &logs {
            assert!(!m.contains("42"), "le client_id ne doit pas fuiter : {m}");
        }
        assert!(
            niveau(log::Level::Info)
                .iter()
                .any(|m| m.contains("connecté à Discord"))
        );
        assert!(p.connecte());
    }

    #[test]
    fn pause_efface_garde_la_connexion_et_la_reprise_attend_15_s() {
        let (journal, h) = (Journal::default(), Horloge::default());
        let mut p = presence(&journal, &h);
        h.regler(100.0);
        p.soumettre(Some(&carte("A")), Some("42"));
        p.effacer_carte();
        assert_eq!(contenu(&journal), ["connecter:42", "publier:A", "effacer"]);
        assert!(p.connecte());
        h.regler(105.0);
        p.soumettre(Some(&carte("A")), Some("42"));
        assert_eq!(compte(&journal, "publier:A"), 1);
        h.regler(115.0);
        p.soumettre(Some(&carte("A")), Some("42"));
        assert_eq!(compte(&journal, "publier:A"), 2);
    }

    #[test]
    fn effacer_carte_sans_connexion_ne_fait_rien() {
        let (journal, h) = (Journal::default(), Horloge::default());
        let mut p = presence(&journal, &h);
        p.effacer_carte();
        assert!(contenu(&journal).is_empty());
        assert!(!p.connecte());
    }

    #[test]
    fn effacer_carte_en_echec_abandonne_la_connexion() {
        let (journal, h) = (Journal::default(), Horloge::default());
        let options = Options {
            echec_effacement: true,
            ..Options::default()
        };
        let mut p = presence_avec(&journal, &h, options, 15.0);
        h.regler(100.0);
        p.soumettre(Some(&carte("A")), Some("42"));
        p.effacer_carte();
        assert!(!p.connecte());
        let j = contenu(&journal);
        assert_eq!(j[j.len() - 2..], ["effacer", "fermer"]);
        h.regler(110.0);
        p.soumettre(Some(&carte("A")), Some("42"));
        assert_eq!(compte(&journal, "connecter:42"), 1);
        h.regler(115.0);
        p.soumettre(Some(&carte("A")), Some("42"));
        assert_eq!(compte(&journal, "connecter:42"), 2);
    }

    // ---- Faux Discord ----

    fn lire_trame(flux: &mut UnixStream) -> (u32, Value) {
        let mut entete = [0u8; 8];
        flux.read_exact(&mut entete).unwrap();
        let op = u32::from_le_bytes(entete[..4].try_into().unwrap());
        let longueur = u32::from_le_bytes(entete[4..].try_into().unwrap());
        let mut corps = vec![0u8; longueur as usize];
        flux.read_exact(&mut corps).unwrap();
        (op, serde_json::from_slice(&corps).unwrap())
    }

    fn ecrire_trame(flux: &mut UnixStream, op: u32, v: &Value) {
        let corps = serde_json::to_vec(v).unwrap();
        let mut trame = op.to_le_bytes().to_vec();
        trame.extend((corps.len() as u32).to_le_bytes());
        trame.extend(corps);
        flux.write_all(&trame).unwrap();
    }

    fn ready() -> Value {
        json!({"cmd": "DISPATCH", "evt": "READY", "data": {}})
    }

    /// Lance un faux Discord : un thread accepte une connexion et la confie au scénario.
    fn faux_discord<T: Send + 'static>(
        scenario: impl FnOnce(UnixStream) -> T + Send + 'static,
    ) -> (tempfile::TempDir, PathBuf, JoinHandle<T>) {
        let dossier = dossier_court();
        let chemin = dossier.path().join("discord-ipc-0");
        let ecouteur = UnixListener::bind(&chemin).unwrap();
        let fil = std::thread::spawn(move || {
            let (flux, _) = ecouteur.accept().unwrap();
            scenario(flux)
        });
        (dossier, chemin, fil)
    }

    /// Scénario standard : poignée de main acceptée, puis une trame lue et acquittée.
    fn client_connecte(chemin: PathBuf) -> ClientIpc {
        let mut c = ClientIpc::new("42", vec![chemin]);
        c.connecter().unwrap();
        c
    }

    #[test]
    fn connecter_envoie_la_poignee_de_main_et_attend_ready() {
        let (_d, chemin, fil) = faux_discord(|mut flux| {
            let trame = lire_trame(&mut flux);
            ecrire_trame(&mut flux, 1, &ready());
            trame
        });
        let mut c = ClientIpc::new("42", vec![chemin]);
        assert_eq!(c.connecter(), Ok(()));
        let (op, json) = fil.join().unwrap();
        assert_eq!(op, 0);
        assert_eq!(json, json!({"v": 1, "client_id": "42"}));
    }

    #[test]
    fn connecter_sans_reponse_expire_en_5_s() {
        let (_d, chemin, fil) = faux_discord(|mut flux| {
            let _ = lire_trame(&mut flux);
            // Ne répond jamais : attend que le client ferme (EOF).
            let mut tampon = [0u8; 1];
            flux.read(&mut tampon).unwrap()
        });
        let mut c = ClientIpc::new("42", vec![chemin]);
        let debut = Instant::now();
        assert_eq!(c.connecter(), Err(DELAI_DEPASSE));
        assert!(debut.elapsed() < Duration::from_secs(6));
        assert_eq!(fil.join().unwrap(), 0, "le socket doit être refermé");
    }

    #[test]
    fn connecter_refuse_par_discord() {
        let (_d, chemin, fil) = faux_discord(|mut flux| {
            let _ = lire_trame(&mut flux);
            ecrire_trame(
                &mut flux,
                2,
                &json!({"code": 4000, "message": "Invalid Client ID"}),
            );
        });
        let mut c = ClientIpc::new("42", vec![chemin]);
        assert_eq!(c.connecter(), Err(REFUSE));
        fil.join().unwrap();
    }

    #[test]
    fn connecter_sans_socket() {
        let dossier = dossier_court();
        let mut c = ClientIpc::new("42", vec![dossier.path().join("discord-ipc-0")]);
        assert_eq!(c.connecter(), Err(AUCUN_SOCKET));
    }

    #[test]
    fn reponse_trop_longue_refusee_sans_allocation() {
        let (_d, chemin, fil) = faux_discord(|mut flux| {
            let _ = lire_trame(&mut flux);
            let mut entete = 1u32.to_le_bytes().to_vec();
            entete.extend((TAILLE_MAX_TRAME + 1).to_le_bytes());
            flux.write_all(&entete).unwrap();
            let mut tampon = [0u8; 1];
            let _ = flux.read(&mut tampon);
        });
        let mut c = ClientIpc::new("42", vec![chemin]);
        assert_eq!(c.connecter(), Err(REPONSE_INVALIDE));
        fil.join().unwrap();
    }

    #[test]
    fn publier_envoie_set_activity_avec_la_carte() {
        let (_d, chemin, fil) = faux_discord(|mut flux| {
            let _ = lire_trame(&mut flux);
            ecrire_trame(&mut flux, 1, &ready());
            let trame = lire_trame(&mut flux);
            ecrire_trame(&mut flux, 1, &json!({"cmd": "SET_ACTIVITY", "evt": null}));
            trame
        });
        let mut c = client_connecte(chemin);
        let mut a = carte("Écrit du code · Projet privé");
        a.debut = 1234;
        assert_eq!(c.publier(&a), Ok(()));
        let (op, json) = fil.join().unwrap();
        assert_eq!(op, 1);
        assert_eq!(json["cmd"], "SET_ACTIVITY");
        assert_eq!(json["nonce"], "1");
        assert_eq!(json["args"]["pid"], std::process::id());
        let activite = &json["args"]["activity"];
        assert_eq!(activite["details"], "Écrit du code · Projet privé");
        assert_eq!(activite["state"], "Opus 5 · 1 k tokens");
        assert_eq!(activite["timestamps"]["start"], 1234);
        assert_eq!(activite["assets"]["large_image"], "logo");
        assert_eq!(activite["assets"]["large_text"], "Claude Code");
    }

    #[test]
    fn publier_refuse_par_discord_sur_evt_error() {
        let (_d, chemin, fil) = faux_discord(|mut flux| {
            let _ = lire_trame(&mut flux);
            ecrire_trame(&mut flux, 1, &ready());
            let _ = lire_trame(&mut flux);
            ecrire_trame(
                &mut flux,
                1,
                &json!({"cmd": "SET_ACTIVITY", "evt": "ERROR"}),
            );
        });
        let mut c = client_connecte(chemin);
        assert_eq!(c.publier(&carte("A")), Err(REFUSE));
        fil.join().unwrap();
    }

    #[test]
    fn effacer_envoie_set_activity_sans_activity() {
        let (_d, chemin, fil) = faux_discord(|mut flux| {
            let _ = lire_trame(&mut flux);
            ecrire_trame(&mut flux, 1, &ready());
            let trame = lire_trame(&mut flux);
            ecrire_trame(&mut flux, 1, &json!({"cmd": "SET_ACTIVITY", "evt": null}));
            trame
        });
        let mut c = client_connecte(chemin);
        assert_eq!(c.effacer(), Ok(()));
        let (op, json) = fil.join().unwrap();
        assert_eq!(op, 1);
        assert_eq!(json["cmd"], "SET_ACTIVITY");
        assert_eq!(json["args"], json!({"pid": std::process::id()}));
    }

    #[test]
    fn publier_apres_fermeture_cote_discord_echoue() {
        let (_d, chemin, fil) = faux_discord(|mut flux| {
            let _ = lire_trame(&mut flux);
            ecrire_trame(&mut flux, 1, &ready());
            // Le flux est abandonné ici : Discord ferme le socket.
        });
        let mut c = client_connecte(chemin);
        fil.join().unwrap();
        assert_eq!(c.publier(&carte("A")), Err(CONNEXION_FERMEE));
    }

    #[test]
    fn fermer_envoie_la_trame_de_fermeture_puis_ferme_le_socket() {
        let (_d, chemin, fil) = faux_discord(|mut flux| {
            let _ = lire_trame(&mut flux);
            ecrire_trame(&mut flux, 1, &ready());
            let trame = lire_trame(&mut flux);
            let mut tampon = [0u8; 1];
            (trame, flux.read(&mut tampon).unwrap())
        });
        let mut c = client_connecte(chemin);
        assert_eq!(c.fermer(), Ok(()));
        let ((op, json), eof) = fil.join().unwrap();
        assert_eq!(op, 2);
        assert_eq!(json, json!({"v": 1, "client_id": "42"}));
        assert_eq!(eof, 0);
    }

    #[test]
    fn fermer_rend_la_premiere_erreur_et_libere_quand_meme_le_socket() {
        // Le pair est parti : l'envoi de la trame de fermeture échoue.
        let (nous, pair) = UnixStream::pair().unwrap();
        drop(pair);
        let mut c = ClientIpc::new("42", vec![]);
        c.flux = Some(nous);
        assert_eq!(c.fermer(), Err(CONNEXION_FERMEE));
        assert!(
            c.flux.is_none(),
            "le socket est libéré même après une erreur"
        );
    }

    #[test]
    fn fermer_sans_connexion_ne_rend_pas_d_erreur() {
        let mut c = ClientIpc::new("42", vec![]);
        assert_eq!(c.fermer(), Ok(()));
    }

    #[test]
    fn chemins_ipc_suit_l_ordre_des_variables() {
        let env = |paires: &'static [(&'static str, &'static str)]| {
            move |nom: &str| {
                paires
                    .iter()
                    .find(|(k, _)| *k == nom)
                    .map(|(_, v)| v.to_string())
            }
        };
        let tous = chemins_ipc_depuis(env(&[
            ("TMPDIR", "/t"),
            ("XDG_RUNTIME_DIR", "/x"),
            ("TEMP", "/e"),
        ]));
        assert_eq!(tous.len(), 10);
        assert_eq!(tous[0], PathBuf::from("/x/discord-ipc-0"));
        assert_eq!(tous[9], PathBuf::from("/x/discord-ipc-9"));
        let tmpdir = chemins_ipc_depuis(env(&[("TMPDIR", "/t"), ("TMP", "/m"), ("TEMP", "/e")]));
        assert_eq!(tmpdir[0], PathBuf::from("/t/discord-ipc-0"));
        let temp = chemins_ipc_depuis(env(&[("TEMP", "/e"), ("TMP", "")]));
        assert_eq!(temp[0], PathBuf::from("/e/discord-ipc-0"));
        let rien = chemins_ipc_depuis(env(&[]));
        assert_eq!(rien[0], PathBuf::from("/tmp/discord-ipc-0"));
    }
}

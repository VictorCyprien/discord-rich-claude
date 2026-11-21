//! Assemblage du daemon : serveur, état, tokens, carte, Discord. Voir la spec, sections 3, 4.4 et 14.2.
//!
//! Verrous : `EtatDaemon` sous un `Mutex` ; `SuiviTokens` et `Presence` chacun à part. Règle :
//! on ne prend jamais `presence` ni `suivi` pendant qu'on tient `EtatDaemon` (la lecture des
//! transcripts et Discord peuvent être lents, les hooks doivent toujours être servis). L'inverse
//! (l'état pris brièvement alors qu'on tient `presence`) est permis : personne ne prend
//! `presence` sous l'état, il n'y a donc pas de cycle.

use std::collections::HashMap;
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::carte::{Carte, Stats, construire_carte, est_vue_b};
use crate::config::ConfigSurveillee;
use crate::journal::panique_rattrapee;
use crate::presence::Presence;
use crate::serveur::{ErreurDemarrage, Gestionnaire, Serveur, demarrer_serveur};
use crate::sessions::{Evenement, Registre};
use crate::tokens::{SuiviTokens, minuit_local};

pub const PERIODE_CONTROLE_S: f64 = 30.0;
pub const PERIODE_CARTE_S: f64 = 1.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Allume,
    Pause,
    Arrete,
}

impl Mode {
    pub fn code(self) -> &'static str {
        match self {
            Mode::Allume => "allume",
            Mode::Pause => "pause",
            Mode::Arrete => "arrete",
        }
    }

    pub fn libelle(self) -> &'static str {
        match self {
            Mode::Allume => "Allumé",
            Mode::Pause => "En pause",
            Mode::Arrete => "Arrêté",
        }
    }
}

/// Horloge murale en secondes depuis l'epoch.
pub fn horloge_systeme() -> Box<dyn Fn() -> f64 + Send + Sync> {
    Box::new(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64())
    })
}

type Rappel = Box<dyn Fn() + Send + Sync>;

type ParSession = HashMap<String, (Option<String>, i64)>;

struct EtatDaemon {
    registre: Registre,
    config: ConfigSurveillee,
    par_session: ParSession,
    tokens_jour: i64,
    mode: Mode,
    premier_controle: bool,
}

pub struct Daemon {
    etat: Mutex<EtatDaemon>,
    suivi: Mutex<SuiviTokens>,
    presence: Mutex<Presence>,
    /// Dernier état de connexion connu : `etat()` ne bloque jamais sur Discord.
    discord_connecte: AtomicBool,
    projets_claude: PathBuf,
    pid_vivant: Box<dyn Fn(i32) -> bool + Send + Sync>,
    horloge: Box<dyn Fn() -> f64 + Send + Sync>,
    /// Appelé quand la config vient d'être rechargée. Verrou de dernier rang : jamais pris
    /// avec un autre verrou du daemon, et le rappel s'exécute sans l'état.
    sur_config_changee: Mutex<Option<Rappel>>,
}

fn verrou<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Daemon {
    pub fn new(
        config: ConfigSurveillee,
        presence: Presence,
        suivi: SuiviTokens,
        projets_claude: PathBuf,
        pid_vivant: Box<dyn Fn(i32) -> bool + Send + Sync>,
        horloge: Box<dyn Fn() -> f64 + Send + Sync>,
    ) -> Arc<Daemon> {
        Arc::new(Daemon {
            etat: Mutex::new(EtatDaemon {
                registre: Registre::default(),
                config,
                par_session: HashMap::new(),
                tokens_jour: 0,
                mode: Mode::Allume,
                premier_controle: true,
            }),
            suivi: Mutex::new(suivi),
            presence: Mutex::new(presence),
            discord_connecte: AtomicBool::new(false),
            projets_claude,
            pid_vivant,
            horloge,
            sur_config_changee: Mutex::new(None),
        })
    }

    pub fn mode(&self) -> Mode {
        verrou(&self.etat).mode
    }

    pub fn chemin_config(&self) -> PathBuf {
        verrou(&self.etat).config.chemin.clone()
    }

    /// Recharge la config si le fichier a changé, puis prévient le rappel. Rend `true` si
    /// elle a changé.
    pub fn recharger_config(&self) -> bool {
        let change = Self::recharger_sous_verrou(&mut verrou(&self.etat));
        if change {
            self.prevenir_config_changee();
        }
        change
    }

    /// Recharge la config (sous le verrou d'état) ; si elle a changé et que `client_id`
    /// manque, journalise l'erreur de la spec 7.1.
    fn recharger_sous_verrou(etat: &mut EtatDaemon) -> bool {
        let change = etat.config.recharger_si_change();
        if change && etat.config.valeur.client_id.is_none() {
            log::error!("client_id absent : pas de connexion à Discord");
        }
        change
    }

    /// Enregistre le rappel appelé après chaque rechargement de la config.
    pub fn sur_config_changee(&self, rappel: Rappel) {
        *verrou(&self.sur_config_changee) = Some(rappel);
    }

    /// Appelle le rappel, sans tenir d'autre verrou que celui du rappel lui-même.
    fn prevenir_config_changee(&self) {
        if let Some(rappel) = verrou(&self.sur_config_changee).as_ref() {
            rappel();
        }
    }

    /// `Debug` des sessions, pour les tests de confidentialité.
    #[doc(hidden)]
    pub fn debug_sessions(&self) -> String {
        format!("{:?}", verrou(&self.etat).registre.sessions)
    }

    /// Tokens du jour calculés au dernier tour de contrôle, pour les tests.
    #[doc(hidden)]
    pub fn tokens_jour(&self) -> i64 {
        verrou(&self.etat).tokens_jour
    }

    pub fn recevoir(&self, ev: Evenement) {
        let mut etat = verrou(&self.etat);
        if etat.mode == Mode::Arrete {
            return;
        }
        let debut: String = ev.session_id.chars().take(8).collect();
        if ev.nom == "SessionStart" || ev.nom == "SessionEnd" {
            log::info!("{} session={debut}", ev.nom);
        } else {
            log::debug!("{} session={debut}", ev.nom);
        }
        etat.registre.appliquer(&ev);
    }

    pub fn etat(&self) -> Value {
        let (sessions, mode) = {
            let etat = verrou(&self.etat);
            (etat.registre.sessions.len(), etat.mode)
        };
        json!({
            "discord": self.discord_connecte.load(Ordering::SeqCst),
            "sessions": sessions,
            "etat": mode.code(),
        })
    }

    pub fn carte(&self) -> Option<Carte> {
        let etat = verrou(&self.etat);
        self.carte_depuis(&etat)
    }

    fn carte_depuis(&self, etat: &EtatDaemon) -> Option<Carte> {
        let maintenant = (self.horloge)();
        let cfg = &etat.config.valeur;
        let session = etat.registre.session_affichee();
        let (modele, tokens) = session
            .and_then(|s| etat.par_session.get(&s.session_id))
            .cloned()
            .unwrap_or((None, 0));
        let stats = Stats {
            modele,
            tokens_session: tokens,
            tokens_jour: etat.tokens_jour,
        };
        construire_carte(
            &etat.registre,
            cfg,
            &stats,
            maintenant,
            est_vue_b(maintenant, cfg.rotation_s),
        )
    }

    /// Un tour de contrôle : config, purge, tokens. Trois temps pour ne tenir le verrou
    /// d'état ni pendant la lecture des transcripts ni avec le verrou du suivi.
    pub fn controler(&self) {
        // 1. sous le verrou d'état
        let (transcripts, minuit, config_changee) = {
            let mut etat = verrou(&self.etat);
            let maintenant = (self.horloge)();
            let change = Self::recharger_sous_verrou(&mut etat);
            // Spec 7.1 : client_id absent → erreur dans le log, une fois au démarrage
            // (un rechargement est déjà journalisé par `recharger_sous_verrou`).
            if etat.config.valeur.client_id.is_none() && etat.premier_controle && !change {
                log::error!("client_id absent : pas de connexion à Discord");
            }
            etat.premier_controle = false;
            // À l'arrêt : ni purge, ni lecture de transcript, ni calcul de tokens.
            if etat.mode == Mode::Arrete {
                drop(etat);
                if change {
                    self.prevenir_config_changee();
                }
                return;
            }
            let inactif_s = etat.config.valeur.inactif_apres_min * 60.0;
            etat.registre
                .purger(maintenant, inactif_s, |pid| (self.pid_vivant)(pid));
            let transcripts: Vec<(String, PathBuf)> = etat
                .registre
                .sessions
                .iter()
                .filter_map(|(sid, s)| {
                    s.transcript_path
                        .as_deref()
                        .filter(|t| !t.is_empty())
                        .map(|t| (sid.clone(), PathBuf::from(t)))
                })
                .collect();
            (transcripts, minuit_local(maintenant), change)
        };
        if config_changee {
            self.prevenir_config_changee();
        }
        // 2. hors du verrou d'état : lecture disque
        let (par_session, tokens_jour) = {
            let mut suivi = verrou(&self.suivi);
            let chemins: Vec<PathBuf> = transcripts.iter().map(|(_, c)| c.clone()).collect();
            suivi.rafraichir(&chemins, minuit);
            let par_session: ParSession = transcripts
                .iter()
                .map(|(sid, t)| (sid.clone(), (suivi.modele(t), suivi.tokens_session(t))))
                .collect();
            (par_session, suivi.tokens_jour(minuit))
        };
        // 3. ranger les résultats
        let mut etat = verrou(&self.etat);
        etat.par_session = par_session;
        etat.tokens_jour = tokens_jour;
    }

    /// Un tour de la boucle carte : calcule la carte puis la soumet à Discord, hors du verrou
    /// d'état. En pause comme à l'arrêt, ne soumet rien.
    pub fn publier(&self) {
        let (carte, client_id) = {
            let etat = verrou(&self.etat);
            if etat.mode != Mode::Allume {
                return;
            }
            (
                self.carte_depuis(&etat),
                etat.config.valeur.client_id.clone(),
            )
        };
        let mut presence = verrou(&self.presence);
        // Un changement de mode peut avoir eu lieu pendant l'attente du verrou : il attend
        // maintenant que `presence` soit libre pour effacer, il ne faut donc pas republier.
        if verrou(&self.etat).mode != Mode::Allume {
            return;
        }
        presence.soumettre(carte.as_ref(), client_id.as_deref());
        self.discord_connecte
            .store(presence.connecte(), Ordering::SeqCst);
    }

    /// Change de mode (spec 14.2) ; agit à la transition.
    pub fn changer_mode(&self, mode: Mode) {
        let precedent = {
            let mut etat = verrou(&self.etat);
            std::mem::replace(&mut etat.mode, mode)
        };
        if precedent == mode {
            return;
        }
        log::info!("mode : {}", mode.libelle());
        match mode {
            Mode::Allume => {}
            Mode::Pause => {
                let mut presence = verrou(&self.presence);
                presence.effacer_carte();
                self.discord_connecte
                    .store(presence.connecte(), Ordering::SeqCst);
            }
            Mode::Arrete => {
                {
                    let mut presence = verrou(&self.presence);
                    presence.fermer();
                    self.discord_connecte.store(false, Ordering::SeqCst);
                }
                let mut etat = verrou(&self.etat);
                etat.registre.sessions.clear();
                etat.par_session.clear();
                etat.tokens_jour = 0;
            }
        }
    }

    fn fermer_presence(&self) {
        let mut presence = verrou(&self.presence);
        presence.fermer();
        self.discord_connecte.store(false, Ordering::SeqCst);
    }
}

impl Gestionnaire for Daemon {
    fn sur_evenement(&self, ev: Evenement) -> Result<(), &'static str> {
        self.recevoir(ev);
        Ok(())
    }

    fn etat(&self) -> Value {
        Daemon::etat(self)
    }
}

/// Suit les tours d'une boucle : une même erreur n'est signalée qu'une fois en ERROR, puis
/// le rétablissement une fois en INFO. Seul le genre de l'erreur est gardé, jamais son contenu
/// (spec 7.5).
#[derive(Default)]
struct Signaleur {
    en_cours: Option<&'static str>,
}

impl Signaleur {
    /// Rend la ligne à journaliser, avec son niveau.
    fn suivre(
        &mut self,
        nom: &str,
        resultat: Result<(), &'static str>,
    ) -> Option<(log::Level, String)> {
        match resultat {
            Err(genre) if self.en_cours != Some(genre) => {
                self.en_cours = Some(genre);
                Some((log::Level::Error, format!("{nom} en erreur : {genre}")))
            }
            Err(_) => None,
            Ok(()) => self
                .en_cours
                .take()
                .map(|_| (log::Level::Info, format!("{nom} rétabli"))),
        }
    }
}

/// Nom, période en secondes, action d'une boucle.
type Tache = (&'static str, f64, fn(&Daemon));

type Arret = Arc<(Mutex<bool>, Condvar)>;

/// Attend `duree` ou le signal d'arrêt ; rend `true` si l'arrêt est demandé.
fn dormir(arret: &Arret, duree: Duration) -> bool {
    let (drapeau, cv) = &**arret;
    let garde = verrou(drapeau);
    let (garde, _) = cv
        .wait_timeout_while(garde, duree, |arrete| !*arrete)
        .unwrap_or_else(|e| e.into_inner());
    *garde
}

fn demander_arret(arret: &Arret) {
    let (drapeau, cv) = &**arret;
    *verrou(drapeau) = true;
    cv.notify_all();
}

fn boucle(arret: &Arret, periode_s: f64, nom: &str, action: impl Fn() -> Result<(), &'static str>) {
    let mut signaleur = Signaleur::default();
    loop {
        if *verrou(&arret.0) {
            return;
        }
        let resultat = {
            // la panique rattrapée est signalée une fois par `Signaleur` : le hook se tait
            let _garde = panique_rattrapee();
            catch_unwind(AssertUnwindSafe(&action)).unwrap_or(Err("panique"))
        };
        if let Some((niveau, texte)) = signaleur.suivre(nom, resultat) {
            log::log!(niveau, "{texte}");
        }
        if dormir(arret, Duration::from_secs_f64(periode_s)) {
            return;
        }
    }
}

pub struct EnMarche {
    serveur: Serveur,
    daemon: Arc<Daemon>,
    arret: Arret,
    fils: Vec<JoinHandle<()>>,
    chemin_socket: PathBuf,
}

fn lancer(
    nom: &'static str,
    daemon: &Arc<Daemon>,
    arret: &Arret,
    periode_s: f64,
    action: fn(&Daemon),
) -> io::Result<JoinHandle<()>> {
    let daemon = Arc::clone(daemon);
    let arret = Arc::clone(arret);
    thread::Builder::new().name(nom.into()).spawn(move || {
        boucle(&arret, periode_s, nom, || {
            action(&daemon);
            Ok(())
        })
    })
}

/// Lie le socket (avant les threads : le `umask` posé pendant le `bind` vaut pour tout le
/// processus), puis lance le contrôle (premier tour immédiat) et la carte.
pub fn demarrer(daemon: &Arc<Daemon>, chemin_socket: &Path) -> Result<EnMarche, ErreurDemarrage> {
    let serveur = demarrer_serveur(
        chemin_socket,
        Arc::clone(daemon) as Arc<dyn Gestionnaire>,
        daemon.projets_claude.clone(),
    )?;
    log::info!("daemon à l'écoute sur {}", chemin_socket.display());
    let arret: Arret = Arc::new((Mutex::new(false), Condvar::new()));
    let mut fils = Vec::new();
    let taches: [Tache; 2] = [
        ("contrôle", PERIODE_CONTROLE_S, Daemon::controler),
        ("carte", PERIODE_CARTE_S, Daemon::publier),
    ];
    for (nom, periode_s, action) in taches {
        match lancer(nom, daemon, &arret, periode_s, action) {
            Ok(fil) => fils.push(fil),
            Err(e) => {
                demander_arret(&arret);
                for fil in fils {
                    let _ = fil.join();
                }
                serveur.arreter();
                return Err(e.into());
            }
        }
    }
    Ok(EnMarche {
        serveur,
        daemon: Arc::clone(daemon),
        arret,
        fils,
        chemin_socket: chemin_socket.to_path_buf(),
    })
}

impl EnMarche {
    /// Plus de connexions, boucles arrêtées, Discord fermé, socket supprimé.
    pub fn arreter(self) {
        self.serveur.arreter();
        demander_arret(&self.arret);
        for fil in self.fils {
            let _ = fil.join();
        }
        self.daemon.fermer_presence();
        if let Err(e) = std::fs::remove_file(&self.chemin_socket)
            && e.kind() != io::ErrorKind::NotFound
        {
            log::warn!("socket non supprimé : {:?}", e.kind());
        }
        log::info!("daemon arrêté");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn libelles_et_codes_des_modes() {
        assert_eq!(Mode::Allume.code(), "allume");
        assert_eq!(Mode::Pause.code(), "pause");
        assert_eq!(Mode::Arrete.code(), "arrete");
        assert_eq!(Mode::Allume.libelle(), "Allumé");
        assert_eq!(Mode::Pause.libelle(), "En pause");
        assert_eq!(Mode::Arrete.libelle(), "Arrêté");
    }

    #[test]
    fn signaleur_ne_repete_pas_la_meme_erreur_et_signale_le_retablissement() {
        let mut s = Signaleur::default();
        let mut lignes = Vec::new();
        let sequence = [
            Err("panique"),
            Err("panique"),
            Err("panique"),
            Err("panique"),
            Err("panique"),
            Ok(()),
            Err("autre"),
        ];
        for r in sequence {
            if let Some(l) = s.suivre("test", r) {
                lignes.push(l);
            }
        }
        assert_eq!(
            lignes,
            [
                (log::Level::Error, "test en erreur : panique".to_string()),
                (log::Level::Info, "test rétabli".to_string()),
                (log::Level::Error, "test en erreur : autre".to_string()),
            ]
        );
    }

    #[test]
    fn la_boucle_survit_a_une_panique_et_s_arrete_vite() {
        let arret: Arret = Arc::new((Mutex::new(false), Condvar::new()));
        let compte = Arc::new(Mutex::new(0u32));
        let (a, c) = (Arc::clone(&arret), Arc::clone(&compte));
        let fil = thread::spawn(move || {
            boucle(&a, 0.01, "test", || {
                *c.lock().unwrap() += 1;
                panic!("SECRET");
            });
        });
        while *compte.lock().unwrap() < 3 {
            thread::sleep(Duration::from_millis(5));
        }
        let debut = std::time::Instant::now();
        demander_arret(&arret);
        fil.join().unwrap();
        assert!(debut.elapsed() < Duration::from_millis(100));
    }

    const MARQUEUR: &str = "SECRET-/chemin/tres/secret/fourni/par/un/hook";

    /// Enfant des tests du hook et de la boucle : hook installé, logger fichier, une action
    /// qui panique cinq fois puis réussit.
    #[test]
    #[ignore = "lancé en sous-processus par les tests du hook et de la boucle"]
    fn enfant_boucle() {
        let Ok(dossier) = std::env::var("DRC_TEST_DOSSIER") else {
            return;
        };
        crate::journal::configurer_logs_avec(Path::new(&dossier), Some("DEBUG"));
        crate::journal::installer_hook_panique();
        let arret: Arret = Arc::new((Mutex::new(false), Condvar::new()));
        let compte = std::sync::atomic::AtomicUsize::new(0);
        let a = Arc::clone(&arret);
        boucle(&arret, 0.001, "test", || {
            let n = compte.fetch_add(1, Ordering::SeqCst) + 1;
            if n <= 5 {
                panic!("{MARQUEUR}");
            }
            demander_arret(&a);
            Ok(())
        });
    }

    /// Lance `enfant_boucle` ; rend (stderr, contenu de daemon.log).
    fn lancer_enfant_boucle() -> (String, String) {
        let dossier = tempfile::tempdir().unwrap();
        let sortie = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "daemon::tests::enfant_boucle",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("DRC_TEST_DOSSIER", dossier.path())
            .output()
            .unwrap();
        assert!(sortie.status.success(), "l'enfant a échoué");
        let journal = std::fs::read_to_string(dossier.path().join("daemon.log")).unwrap();
        (
            String::from_utf8_lossy(&sortie.stderr).into_owned(),
            journal,
        )
    }

    #[test]
    fn hook_muet_sous_catch_unwind() {
        let (stderr, journal) = lancer_enfant_boucle();
        let erreurs_hook = journal
            .lines()
            .filter(|l| l.contains("erreur interne (panique)"))
            .count();
        assert_eq!(erreurs_hook, 0, "journal : {journal}");
        let erreurs = journal.lines().filter(|l| l.contains(" ERROR ")).count();
        assert_eq!(erreurs, 1, "journal : {journal}");
        assert!(!stderr.contains("SECRET") && !stderr.contains("/chemin/"));
        assert!(!stderr.contains("erreur interne"), "stderr : {stderr}");
    }

    /// Port de `test_boucle_ne_repete_pas_la_meme_erreur_et_signale_le_retablissement`.
    /// Écart accepté : la trace complète en DEBUG n'existe pas en Rust.
    #[test]
    fn test_boucle_ne_repete_pas_la_meme_erreur_et_signale_le_retablissement() {
        let (_, journal) = lancer_enfant_boucle();
        let erreurs: Vec<&str> = journal.lines().filter(|l| l.contains(" ERROR ")).collect();
        let infos: Vec<&str> = journal.lines().filter(|l| l.contains(" INFO ")).collect();
        assert_eq!(erreurs.len(), 1, "journal : {journal}");
        assert!(erreurs[0].ends_with("test en erreur : panique"));
        assert_eq!(infos.len(), 1);
        assert!(infos[0].ends_with("test rétabli"));
        for l in erreurs.iter().chain(infos.iter()) {
            assert!(!l.contains("SECRET") && !l.contains("/chemin/"));
        }
    }

    struct ClientNul;

    impl crate::presence::ClientDiscord for ClientNul {
        fn connecter(&mut self) -> Result<(), crate::presence::ErreurDiscord> {
            Ok(())
        }
        fn publier(&mut self, _: &Carte) -> Result<(), crate::presence::ErreurDiscord> {
            Ok(())
        }
        fn effacer(&mut self) -> Result<(), crate::presence::ErreurDiscord> {
            Ok(())
        }
        fn fermer(&mut self) -> Result<(), crate::presence::ErreurDiscord> {
            Ok(())
        }
    }

    /// Daemon factice sur une config temporaire ; `(dossier, chemin de la config, daemon)`.
    fn daemon_avec_config() -> (tempfile::TempDir, PathBuf, Arc<Daemon>) {
        let dossier = tempfile::tempdir().unwrap();
        let config = dossier.path().join("config.toml");
        std::fs::write(&config, "client_id = \"1\"\n").unwrap();
        let daemon = Daemon::new(
            ConfigSurveillee::new(config.clone()),
            Presence::new(
                Box::new(|_| Box::new(ClientNul)),
                15.0,
                15.0,
                Box::new(|| 0.0),
            ),
            SuiviTokens::new(dossier.path().join("projets")),
            dossier.path().join("projets"),
            Box::new(|_| true),
            Box::new(|| 0.0),
        );
        (dossier, config, daemon)
    }

    fn compteur_de_rappels(daemon: &Daemon) -> Arc<std::sync::atomic::AtomicUsize> {
        let compte = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = Arc::clone(&compte);
        daemon.sur_config_changee(Box::new(move || {
            c.fetch_add(1, Ordering::SeqCst);
        }));
        compte
    }

    #[test]
    fn rappel_appele_quand_la_config_change_au_controle() {
        let (_dossier, config, daemon) = daemon_avec_config();
        let compte = compteur_de_rappels(&daemon);
        std::fs::write(&config, "client_id = \"2\"\n").unwrap();
        daemon.controler();
        assert_eq!(compte.load(Ordering::SeqCst), 1);
        daemon.controler();
        assert_eq!(compte.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn rappel_appele_aussi_en_mode_arrete() {
        let (_dossier, config, daemon) = daemon_avec_config();
        let compte = compteur_de_rappels(&daemon);
        daemon.changer_mode(Mode::Arrete);
        std::fs::write(&config, "client_id = \"2\"\n").unwrap();
        daemon.controler();
        assert_eq!(compte.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn recharger_config_sans_changement_rend_faux() {
        let (_dossier, config, daemon) = daemon_avec_config();
        let compte = compteur_de_rappels(&daemon);
        assert!(!daemon.recharger_config());
        assert_eq!(compte.load(Ordering::SeqCst), 0);
        assert_eq!(daemon.chemin_config(), config);
        std::fs::write(&config, "client_id = \"2\"\n").unwrap();
        assert!(daemon.recharger_config());
        assert_eq!(compte.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn recharger_config_sans_client_id_garde_le_comportement() {
        let (_dossier, config, daemon) = daemon_avec_config();
        std::fs::write(&config, "inactif_apres_min = 5\n").unwrap();
        assert!(daemon.recharger_config());
    }
}

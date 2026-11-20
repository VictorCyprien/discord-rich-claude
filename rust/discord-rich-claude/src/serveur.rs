//! Réception des hooks sur un socket Unix. Voir la spec, sections 3, 7.3 et 7.5.
//!
//! HTTP minimal codé à la main : une route d'écriture, une de lecture, rien d'autre.
//! Le JSON d'un hook contient le prompt et les entrées d'outil : `filtrer` n'en
//! garde qu'une liste fermée de champs, le reste n'est jamais copié ni loggé.
//!
//! Un thread par connexion. Les logs ne contiennent jamais le corps, un en-tête,
//! un chemin ni une trace : seulement un genre d'erreur (`&'static str`).

use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde::de::{self, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};

use crate::chemins::resoudre;
use crate::sessions::Evenement;

/// Un PreToolUse sur Write transporte tout le fichier.
pub const TAILLE_MAX: usize = 16 * 1024 * 1024;
pub const DELAI: Duration = Duration::from_secs(2);
const PID_MAX: i64 = 1 << 31;
/// Limite de l'en-tête (le défaut de `asyncio.StreamReader`).
const LIMITE_ENTETE: usize = 64 * 1024;
/// Limite de chiffres de `int()` en Python (3.11+).
const CHIFFRES_MAX: usize = 4300;
const EVENEMENTS: [&str; 8] = [
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "Stop",
    "SubagentStart",
    "SubagentStop",
    "SessionEnd",
];

/// Ce que le serveur demande au reste du daemon.
pub trait Gestionnaire: Send + Sync {
    /// Traite un événement filtré. L'erreur est un genre fixe, jamais un contenu.
    fn sur_evenement(&self, ev: Evenement) -> Result<(), &'static str>;
    /// État rendu par `GET /etat`.
    fn etat(&self) -> serde_json::Value;
}

#[derive(Debug)]
pub enum ErreurDemarrage {
    /// Un autre daemon écoute déjà sur le socket.
    DejaLance(PathBuf),
    Io(io::Error),
}

impl fmt::Display for ErreurDemarrage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DejaLance(chemin) => write!(f, "un daemon écoute déjà sur {}", chemin.display()),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ErreurDemarrage {}

impl From<io::Error> for ErreurDemarrage {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

// ---------------------------------------------------------------------------
// Filtrage
// ---------------------------------------------------------------------------

/// `Path.suffix == ".jsonl"` : le nom doit finir par `.jsonl` et avoir un radical.
fn suffixe_jsonl(chemin: &Path) -> bool {
    chemin
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.len() > ".jsonl".len() && n.ends_with(".jsonl"))
}

pub fn transcript_autorise(chemin: &str, racine: &Path) -> bool {
    if chemin.contains('\0') {
        return false;
    }
    let p = Path::new(chemin);
    if !p.is_absolute() || !suffixe_jsonl(p) {
        return false;
    }
    let resolu = match resoudre(p, true) {
        Ok(r) => r,
        // le transcript peut ne pas encore exister (ex. SessionStart avant écriture)
        Err(e) if e.kind() == io::ErrorKind::NotFound => match resoudre(p, false) {
            Ok(r) => r,
            Err(_) => return false,
        },
        // boucle de liens, droits, NUL : refusé
        Err(_) => return false,
    };
    match resoudre(racine, false) {
        Ok(racine) => resolu.ancestors().skip(1).any(|a| a == racine),
        Err(_) => false,
    }
}

fn pid(brut: Option<&str>) -> Option<i32> {
    let brut = brut?;
    if brut.is_empty()
        || !brut.bytes().all(|b| b.is_ascii_digit())
        || !(1..=7).contains(&brut.len())
    {
        return None;
    }
    let pid: i64 = brut.parse().ok()?;
    if 1 < pid && pid < PID_MAX {
        i32::try_from(pid).ok()
    } else {
        None
    }
}

fn cwd(brut: Option<String>) -> Option<String> {
    brut.filter(|c| !c.contains('\0') && c.starts_with('/'))
}

/// `^[A-Za-z0-9_-]{1,128}$` avec la sémantique de `fullmatch` (aucun `\n` final).
fn identifiant_valide(s: &str) -> bool {
    (1..=128).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn identifiant(brut: Option<String>) -> Option<String> {
    brut.filter(|s| identifiant_valide(s))
}

/// La valeur d'un champ lu : absente (ou `null`), textuelle, ou d'un autre type.
#[derive(Default)]
enum Champ {
    #[default]
    Absent,
    Texte(String),
    Autre,
}

impl Champ {
    fn texte(&mut self) -> Option<String> {
        match std::mem::take(self) {
            Self::Texte(t) => Some(t),
            _ => None,
        }
    }
}

struct VisiteurChamp;

impl<'de> Visitor<'de> for VisiteurChamp {
    type Value = Champ;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("une valeur JSON")
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<Champ, E> {
        Ok(Champ::Texte(v.to_owned()))
    }
    fn visit_string<E: de::Error>(self, v: String) -> Result<Champ, E> {
        Ok(Champ::Texte(v))
    }
    fn visit_unit<E: de::Error>(self) -> Result<Champ, E> {
        Ok(Champ::Absent)
    }
    fn visit_none<E: de::Error>(self) -> Result<Champ, E> {
        Ok(Champ::Absent)
    }
    fn visit_bool<E: de::Error>(self, _: bool) -> Result<Champ, E> {
        Ok(Champ::Autre)
    }
    fn visit_i64<E: de::Error>(self, _: i64) -> Result<Champ, E> {
        Ok(Champ::Autre)
    }
    fn visit_u64<E: de::Error>(self, _: u64) -> Result<Champ, E> {
        Ok(Champ::Autre)
    }
    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Champ, E> {
        Ok(Champ::Autre)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Champ, A::Error> {
        while seq.next_element::<IgnoredAny>()?.is_some() {}
        Ok(Champ::Autre)
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Champ, A::Error> {
        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(Champ::Autre)
    }
}

impl<'de> Deserialize<'de> for Champ {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(VisiteurChamp)
    }
}

/// Les six champs de la liste fermée, rien d'autre.
/// L'ordre : `session_id`, `cwd`, `hook_event_name`, `tool_name`, `agent_id`, `transcript_path`.
const CHAMPS_LUS: [&str; 6] = [
    "session_id",
    "cwd",
    "hook_event_name",
    "tool_name",
    "agent_id",
    "transcript_path",
];

#[derive(Default)]
struct ChampsLus([Champ; 6]);

struct Cle(Option<usize>);

impl<'de> Deserialize<'de> for Cle {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = Cle;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("une clé")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Cle, E> {
                Ok(Cle(CHAMPS_LUS.iter().position(|c| *c == v)))
            }
        }
        d.deserialize_str(V)
    }
}

impl<'de> Deserialize<'de> for ChampsLus {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = ChampsLus;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("un objet JSON")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<ChampsLus, A::Error> {
                let mut champs = ChampsLus::default();
                // En cas de doublon, la dernière clé gagne, comme `json.loads`.
                while let Some(Cle(indice)) = map.next_key()? {
                    match indice {
                        Some(i) => champs.0[i] = map.next_value()?,
                        None => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Ok(champs)
            }
        }
        d.deserialize_map(V)
    }
}

/// Ne garde que la liste fermée de champs ; rend `None` sur toute entrée invalide.
pub fn filtrer(
    corps: &[u8],
    pid_brut: Option<&str>,
    recu_a: f64,
    projets_claude: &Path,
) -> Option<Evenement> {
    let ChampsLus(mut champs) = serde_json::from_slice(corps).ok()?;
    // un champ présent mais non textuel (et non null) fait rejeter l'événement entier
    if champs.iter().any(|c| matches!(c, Champ::Autre)) {
        return None;
    }
    let session_id = champs[0].texte();
    let cwd_brut = champs[1].texte();
    let nom = champs[2].texte();
    let tool_name = champs[3].texte();
    let agent_id = champs[4].texte();
    let transcript = champs[5].texte();

    let nom = nom.filter(|n| EVENEMENTS.contains(&n.as_str()))?;
    let session_id = session_id.filter(|s| identifiant_valide(s))?;
    let transcript = transcript.filter(|t| transcript_autorise(t, projets_claude));
    Some(Evenement {
        nom,
        session_id,
        recu_a,
        cwd: cwd(cwd_brut),
        tool_name,
        agent_id: identifiant(agent_id),
        transcript_path: transcript,
        pid: pid(pid_brut),
    })
}

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------

/// Pourquoi une requête s'arrête. Jamais de contenu : un genre fixe.
enum ErreurRequete {
    /// Requête mal formée, délai dépassé, connexion coupée : pas de réponse, log debug.
    Rejetee(&'static str),
    /// Erreur imprévue : réponse 500 et log erreur.
    Inattendue(&'static str),
}

fn genre_io(e: &io::Error) -> &'static str {
    match e.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => "délai dépassé",
        io::ErrorKind::UnexpectedEof => "connexion coupée",
        io::ErrorKind::ConnectionReset
        | io::ErrorKind::ConnectionAborted
        | io::ErrorKind::BrokenPipe => "connexion coupée",
        _ => "erreur d'entrée-sortie",
    }
}

fn raison(code: u16) -> &'static str {
    match code {
        200 => "OK",
        204 => "No Content",
        404 => "Not Found",
        411 => "Length Required",
        413 => "Content Too Large",
        _ => "Internal Server Error",
    }
}

fn repondre(flux: &mut UnixStream, code: u16, corps: &[u8]) -> io::Result<()> {
    let mut tete = format!(
        "HTTP/1.1 {code} {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        raison(code),
        corps.len()
    );
    if !corps.is_empty() {
        tete.push_str("Content-Type: application/json\r\n");
    }
    tete.push_str("\r\n");
    let mut tout = tete.into_bytes();
    tout.extend_from_slice(corps);
    flux.write_all(&tout)
}

/// Une lecture, avec le temps restant avant l'échéance TOTALE (jamais un délai par lecture).
fn lire_avant(flux: &mut UnixStream, tampon: &mut [u8], echeance: Instant) -> io::Result<usize> {
    loop {
        let restant = echeance.saturating_duration_since(Instant::now());
        if restant.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        // macOS refuse (EINVAL) de régler le délai d'un socket dont le pair a déjà fermé : la
        // lecture rend alors tout de suite ce qui reste ou la fin de flux, sans bloquer. Le délai
        // posé à la création de la connexion reste une borne dans tous les cas.
        let _ = flux.set_read_timeout(Some(restant));
        match flux.read(tampon) {
            Ok(n) => return Ok(n),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
}

/// Lit jusqu'à `\r\n\r\n`. Rend l'en-tête (séparateur compris) et ce qui le suit déjà.
fn lire_entete(
    flux: &mut UnixStream,
    echeance: Instant,
) -> Result<(Vec<u8>, Vec<u8>), ErreurRequete> {
    let mut tampon: Vec<u8> = Vec::new();
    let mut bloc = [0u8; 4096];
    let mut depuis = 0;
    loop {
        if let Some(pos) = tampon[depuis..]
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map(|p| p + depuis)
        {
            if pos > LIMITE_ENTETE {
                return Err(ErreurRequete::Rejetee("en-tête trop long"));
            }
            let reste = tampon.split_off(pos + 4);
            return Ok((tampon, reste));
        }
        if tampon.len() > LIMITE_ENTETE {
            return Err(ErreurRequete::Rejetee("en-tête trop long"));
        }
        depuis = tampon.len().saturating_sub(3);
        match lire_avant(flux, &mut bloc, echeance) {
            Ok(0) => return Err(ErreurRequete::Rejetee("connexion coupée")),
            Ok(n) => tampon.extend_from_slice(&bloc[..n]),
            Err(e) => return Err(ErreurRequete::Rejetee(genre_io(&e))),
        }
    }
}

/// `str.strip()` de Python sur du latin-1.
fn blanc_python(c: char) -> bool {
    matches!(c, '\t'..='\r' | '\x1c'..='\x1f' | ' ' | '\u{85}' | '\u{a0}')
}

/// `int(texte)` de Python, saturé : rend `None` là où Python lève `ValueError`.
fn entier_python(texte: &str) -> Option<i128> {
    let (negatif, reste) = match texte.as_bytes().first()? {
        b'+' => (false, &texte[1..]),
        b'-' => (true, &texte[1..]),
        _ => (false, texte),
    };
    let octets = reste.as_bytes();
    let (premier, dernier) = (octets.first()?, octets.last()?);
    if !premier.is_ascii_digit() || !dernier.is_ascii_digit() {
        return None;
    }
    let mut magnitude: u128 = 0;
    let mut chiffres = 0;
    let mut precedent_souligne = false;
    for &b in octets {
        match b {
            b'0'..=b'9' => {
                precedent_souligne = false;
                chiffres += 1;
                magnitude = magnitude
                    .saturating_mul(10)
                    .saturating_add(u128::from(b - b'0'));
            }
            b'_' if !precedent_souligne => precedent_souligne = true,
            _ => return None,
        }
    }
    if chiffres > CHIFFRES_MAX {
        return None;
    }
    let magnitude = i128::try_from(magnitude.min(i128::MAX as u128)).ok()?;
    Some(if negatif { -magnitude } else { magnitude })
}

struct Requete {
    methode: String,
    chemin: String,
    content_length: Option<String>,
    expect: Option<String>,
    pid: Option<String>,
}

/// Décode l'en-tête (latin-1) ; seuls les trois en-têtes utiles sont retenus.
fn analyser(tete: &[u8]) -> Result<Requete, ErreurRequete> {
    let texte: String = tete.iter().map(|&b| char::from(b)).collect();
    let mut lignes = texte.split("\r\n");
    let premiere = lignes.next().unwrap_or("");
    let mut morceaux = premiere.splitn(3, ' ');
    let (Some(methode), Some(chemin), Some(_)) =
        (morceaux.next(), morceaux.next(), morceaux.next())
    else {
        return Err(ErreurRequete::Rejetee("requête mal formée"));
    };
    let mut requete = Requete {
        methode: methode.to_owned(),
        chemin: chemin.to_owned(),
        content_length: None,
        expect: None,
        pid: None,
    };
    for ligne in lignes {
        let Some((cle, valeur)) = ligne.split_once(':') else {
            continue;
        };
        let valeur = valeur.trim_matches(blanc_python).to_owned();
        match cle.trim_matches(blanc_python).to_lowercase().as_str() {
            "content-length" => requete.content_length = Some(valeur),
            "expect" => requete.expect = Some(valeur),
            "x-claude-pid" => requete.pid = Some(valeur),
            _ => {}
        }
    }
    Ok(requete)
}

fn maintenant_s() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

fn traiter(
    flux: &mut UnixStream,
    gestionnaire: &dyn Gestionnaire,
    projets_claude: &Path,
) -> Result<(), ErreurRequete> {
    let io_rejet = |e: io::Error| ErreurRequete::Rejetee(genre_io(&e));
    let (tete, mut corps) = lire_entete(flux, Instant::now() + DELAI)?;
    let requete = analyser(&tete)?;
    drop(tete);

    if requete.methode == "GET" && requete.chemin == "/etat" {
        let etat = catch_unwind(AssertUnwindSafe(|| gestionnaire.etat()))
            .map_err(|_| ErreurRequete::Inattendue("panique"))?;
        let json =
            serde_json::to_string(&etat).map_err(|_| ErreurRequete::Inattendue("sérialisation"))?;
        return repondre(flux, 200, json.as_bytes()).map_err(io_rejet);
    }
    if requete.methode != "POST" || requete.chemin != "/event" {
        return repondre(flux, 404, b"").map_err(io_rejet);
    }
    let Some(brut) = requete.content_length.as_deref() else {
        return repondre(flux, 411, b"").map_err(io_rejet);
    };
    let longueur = entier_python(brut).ok_or(ErreurRequete::Rejetee("requête mal formée"))?;
    let Some(longueur) = usize::try_from(longueur).ok().filter(|&l| l <= TAILLE_MAX) else {
        return repondre(flux, 413, b"").map_err(io_rejet);
    };
    if requete
        .expect
        .as_deref()
        .is_some_and(|e| e.to_lowercase() == "100-continue")
    {
        flux.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")
            .map_err(io_rejet)?;
    }

    // le corps doit arriver dans les 2 s, échéance totale
    let echeance = Instant::now() + DELAI;
    corps.truncate(longueur);
    let mut bloc = vec![0u8; 8192];
    while corps.len() < longueur {
        let voulu = (longueur - corps.len()).min(bloc.len());
        match lire_avant(flux, &mut bloc[..voulu], echeance) {
            Ok(0) => return Err(ErreurRequete::Rejetee("connexion coupée")),
            Ok(n) => corps.extend_from_slice(&bloc[..n]),
            Err(e) => return Err(io_rejet(e)),
        }
    }
    let ev = filtrer(
        &corps,
        requete.pid.as_deref(),
        maintenant_s(),
        projets_claude,
    );
    drop(corps);
    if let Some(ev) = ev {
        let nom = ev.nom.clone();
        let resultat = catch_unwind(AssertUnwindSafe(|| gestionnaire.sur_evenement(ev)))
            .unwrap_or(Err("panique"));
        if let Err(genre) = resultat {
            log::error!("erreur sur un événement {nom} : {genre}");
            return repondre(flux, 500, b"").map_err(io_rejet);
        }
    }
    repondre(flux, 204, b"").map_err(io_rejet)
}

fn servir(mut flux: UnixStream, gestionnaire: &dyn Gestionnaire, projets_claude: &Path) {
    // bornes de départ ; `lire_avant` les resserre à chaque lecture
    if flux.set_read_timeout(Some(DELAI)).is_err() || flux.set_write_timeout(Some(DELAI)).is_err() {
        log::debug!("requête rejetée : connexion coupée");
        return;
    }
    match traiter(&mut flux, gestionnaire, projets_claude) {
        Ok(()) => {}
        Err(ErreurRequete::Rejetee(genre)) => log::debug!("requête rejetée : {genre}"),
        Err(ErreurRequete::Inattendue(genre)) => {
            // dernier filet : ni contenu ni trace dans le log
            log::error!("erreur inattendue dans le traitement d'une requête : {genre}");
            let _ = repondre(&mut flux, 500, b"");
        }
    }
    let _ = flux.shutdown(Shutdown::Both);
}

// ---------------------------------------------------------------------------
// Démarrage
// ---------------------------------------------------------------------------

fn verifier_dossier_etat(dossier: &Path) -> Result<(), ErreurDemarrage> {
    fs::create_dir_all(dossier)?;
    let info = fs::symlink_metadata(dossier)?;
    // SAFETY : `geteuid` n'a aucun effet de bord.
    let uid = unsafe { libc::geteuid() };
    if info.file_type().is_symlink() || !info.is_dir() || info.uid() != uid {
        return Err(ErreurDemarrage::Io(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "dossier d'état invalide (lien, non-dossier ou autre propriétaire) : {}",
                dossier.display()
            ),
        )));
    }
    fs::set_permissions(dossier, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn remplacer_ancien_socket(chemin: &Path) -> Result<(), ErreurDemarrage> {
    let info = match fs::symlink_metadata(chemin) {
        Ok(i) => i,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    if !info.file_type().is_socket() {
        // lien ou fichier ordinaire à la place du socket : remove_file ne suit jamais un lien
        fs::remove_file(chemin)?;
        return Ok(());
    }
    if UnixStream::connect(chemin).is_ok() {
        return Err(ErreurDemarrage::DejaLance(chemin.to_path_buf()));
    }
    fs::remove_file(chemin)?; // socket orphelin d'un daemon mort
    Ok(())
}

/// Pose un umask et le restaure à la sortie, même en cas d'erreur ou de panique.
struct GardeUmask(libc::mode_t);

impl GardeUmask {
    fn poser(masque: libc::mode_t) -> Self {
        // SAFETY : `umask` ne touche qu'au masque du processus.
        Self(unsafe { libc::umask(masque) })
    }
}

impl Drop for GardeUmask {
    fn drop(&mut self) {
        // SAFETY : voir `poser`.
        unsafe { libc::umask(self.0) };
    }
}

pub struct Serveur {
    chemin: PathBuf,
    arret: Arc<AtomicBool>,
    fil: Option<JoinHandle<()>>,
}

impl Serveur {
    /// Arrête la boucle `accept` et attend son thread. Les connexions en cours finissent
    /// seules. Ne supprime pas le socket.
    pub fn arreter(self) {
        drop(self);
    }
}

impl Drop for Serveur {
    fn drop(&mut self) {
        let Some(fil) = self.fil.take() else {
            return;
        };
        self.arret.store(true, Ordering::SeqCst);
        // une connexion à soi-même débloque `accept` ; sans elle, on ne peut pas attendre
        if UnixStream::connect(&self.chemin).is_ok() {
            let _ = fil.join();
        }
    }
}

pub fn demarrer_serveur(
    chemin_socket: &Path,
    gestionnaire: Arc<dyn Gestionnaire>,
    projets_claude: PathBuf,
) -> Result<Serveur, ErreurDemarrage> {
    let dossier = match chemin_socket.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    verifier_dossier_etat(dossier)?;
    remplacer_ancien_socket(chemin_socket)?;
    let ecouteur = {
        let _garde = GardeUmask::poser(0o177);
        UnixListener::bind(chemin_socket)?
    };
    fs::set_permissions(chemin_socket, fs::Permissions::from_mode(0o600))?;

    let arret = Arc::new(AtomicBool::new(false));
    let arret_fil = Arc::clone(&arret);
    let projets = Arc::new(projets_claude);
    let fil = thread::Builder::new()
        .name("serveur".into())
        .spawn(move || {
            for connexion in ecouteur.incoming() {
                if arret_fil.load(Ordering::SeqCst) {
                    break;
                }
                match connexion {
                    Ok(flux) => {
                        let gestionnaire = Arc::clone(&gestionnaire);
                        let projets = Arc::clone(&projets);
                        let lance = thread::Builder::new()
                            .name("connexion".into())
                            .spawn(move || servir(flux, gestionnaire.as_ref(), &projets));
                        if lance.is_err() {
                            log::debug!("requête rejetée : fil indisponible");
                        }
                    }
                    Err(e) => {
                        log::debug!("accept refusé : {}", genre_io(&e));
                        thread::sleep(Duration::from_millis(50));
                    }
                }
            }
        })?;
    Ok(Serveur {
        chemin: chemin_socket.to_path_buf(),
        arret,
        fil: Some(fil),
    })
}

/// Interroge `GET /etat` ; `None` si le daemon ne répond pas ou répond autre chose qu'un 200 JSON.
pub fn interroger_etat(chemin_socket: &Path, delai: Duration) -> Option<serde_json::Value> {
    let echeance = Instant::now() + delai;
    let mut flux = UnixStream::connect(chemin_socket).ok()?;
    let borne = delai.max(Duration::from_millis(1));
    flux.set_write_timeout(Some(borne)).ok()?;
    flux.set_read_timeout(Some(borne)).ok()?;
    flux.write_all(b"GET /etat HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .ok()?;
    let mut reponse = Vec::new();
    let mut bloc = [0u8; 4096];
    loop {
        match lire_avant(&mut flux, &mut bloc, echeance) {
            Ok(0) => break,
            Ok(n) => {
                reponse.extend_from_slice(&bloc[..n]);
                if reponse.len() > TAILLE_MAX {
                    return None;
                }
            }
            Err(_) => return None,
        }
    }
    let separation = reponse.windows(4).position(|w| w == b"\r\n\r\n");
    let (tete, corps) = match separation {
        Some(p) => (&reponse[..p], &reponse[p + 4..]),
        None => (&reponse[..], &[][..]),
    };
    if !tete.starts_with(b"HTTP/1.1 200") {
        return None;
    }
    serde_json::from_slice(corps).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::os::unix::fs::symlink;

    // --- filtrer ---

    fn corps(valeur: serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(&valeur).unwrap()
    }

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn filtrer_ne_garde_que_la_liste_fermee() {
        let t = tmp();
        let ev = filtrer(
            &corps(json!({
                "hook_event_name": "PreToolUse",
                "session_id": "abc-123",
                "cwd": "/p",
                "tool_name": "Write",
                "tool_input": {"file_path": "/secret/fichier.py", "content": "SECRET-CONTENU"},
                "prompt": "SECRET-PROMPT",
                "last_assistant_message": "SECRET-REPONSE",
            })),
            Some("4242"),
            10.0,
            t.path(),
        )
        .unwrap();
        assert_eq!(ev.nom, "PreToolUse");
        assert_eq!(ev.session_id, "abc-123");
        assert_eq!(ev.cwd.as_deref(), Some("/p"));
        assert_eq!(ev.tool_name.as_deref(), Some("Write"));
        assert_eq!(ev.pid, Some(4242));
        assert_eq!(ev.recu_a, 10.0);
        assert!(!format!("{ev:?}").contains("SECRET"));
    }

    fn refuse(brut: &[u8]) {
        let t = tmp();
        assert!(filtrer(brut, None, 0.0, t.path()).is_none(), "{brut:?}");
    }

    #[test]
    fn filtrer_refuse() {
        refuse(b"{pas du json");
        refuse(b"\xff\xfe");
        refuse(b"[1, 2]");
        refuse(&corps(
            json!({"hook_event_name": "Notification", "session_id": "s1"}),
        ));
        refuse(&corps(json!({"hook_event_name": "Stop"})));
        refuse(&corps(json!({"hook_event_name": "Stop", "session_id": ""})));
        refuse(&corps(
            json!({"hook_event_name": "Stop", "session_id": "../../etc"}),
        ));
        refuse(&corps(
            json!({"hook_event_name": "Stop", "session_id": "s1", "tool_name": 3}),
        ));
        // profondeur de récursion : jamais laissée s'échapper comme du JSON valide
        refuse(&vec![b'['; 100_000]);
        // `$` accepte un `\n` final, `fullmatch` non
        refuse(&corps(
            json!({"hook_event_name": "Stop", "session_id": "s1\n"}),
        ));
    }

    #[test]
    fn filtrer_refuse_un_champ_non_textuel_meme_inutile() {
        // un champ de la liste fermée présent mais non textuel fait rejeter l'événement
        for autre in [
            json!(1),
            json!(1.5),
            json!(true),
            json!([]),
            json!({"a": 1}),
        ] {
            for champ in ["cwd", "agent_id", "transcript_path", "tool_name"] {
                let mut o = json!({"hook_event_name": "Stop", "session_id": "s1"});
                o[champ] = autre.clone();
                refuse(&corps(o));
            }
        }
    }

    #[test]
    fn filtrer_accepte_null_comme_absent() {
        let t = tmp();
        let ev = filtrer(
            &corps(json!({"hook_event_name": "Stop", "session_id": "s1", "cwd": null, "tool_name": null})),
            None,
            0.0,
            t.path(),
        )
        .unwrap();
        assert_eq!((ev.cwd, ev.tool_name), (None, None));
    }

    #[test]
    fn filtrer_derniere_cle_gagnante() {
        let t = tmp();
        let ev = filtrer(
            br#"{"hook_event_name":"Stop","session_id":"a","session_id":"b","cwd":3,"cwd":"/x"}"#,
            None,
            0.0,
            t.path(),
        )
        .unwrap();
        assert_eq!(ev.session_id, "b");
        // la clé dupliquée non textuelle est écrasée : l'événement n'est pas rejeté
        assert_eq!(ev.cwd.as_deref(), Some("/x"));
        // et l'inverse : le dernier gagne, même s'il est invalide
        refuse(br#"{"hook_event_name":"Stop","session_id":"a","session_id":"../b"}"#);
        refuse(br#"{"hook_event_name":"Stop","session_id":"a","cwd":"/x","cwd":3}"#);
    }

    #[test]
    fn filtrer_ignore_les_champs_inconnus_imbriques() {
        let t = tmp();
        let ev = filtrer(
            br#"{"tool_input":{"a":[1,2,{"b":null}],"c":"d"},"hook_event_name":"Stop","session_id":"s1","x":[[[]]]}"#,
            None,
            0.0,
            t.path(),
        );
        assert!(ev.is_some());
    }

    #[test]
    fn filtrer_profondeur() {
        let t = tmp();
        let imbrique = |n: usize| {
            let mut brut = br#"{"hook_event_name":"Stop","session_id":"s1","x":"#.to_vec();
            brut.extend(vec![b'['; n]);
            brut.extend(vec![b']'; n]);
            brut.push(b'}');
            brut
        };
        // un champ inconnu est sauté sans récursion : 200 niveaux passent, comme en Python
        assert!(filtrer(&imbrique(200), None, 0.0, t.path()).is_some());
        // une profondeur démesurée ne fait jamais déborder la pile
        let _ = filtrer(&imbrique(1_000_000), None, 0.0, t.path());
        // un champ de la liste fermée imbriqué est de toute façon non textuel : rejeté
        let mut brut = br#"{"hook_event_name":"Stop","session_id":"s1","tool_name":"#.to_vec();
        brut.extend(vec![b'['; 200]);
        brut.extend(vec![b']'; 200]);
        brut.push(b'}');
        assert!(filtrer(&brut, None, 0.0, t.path()).is_none());
    }

    fn avec_pid(pid_brut: Option<&str>) -> Option<i32> {
        let t = tmp();
        filtrer(
            &corps(json!({"hook_event_name": "Stop", "session_id": "s1"})),
            pid_brut,
            0.0,
            t.path(),
        )
        .unwrap()
        .pid
    }

    #[test]
    fn filtrer_pid() {
        assert_eq!(avec_pid(Some("123")), Some(123));
        assert_eq!(avec_pid(None), None);
        for refuse in [
            "abc",
            "1",
            "0",
            "１２",
            "",
            "99999999999",
            "-5",
            "+5",
            " 12",
            "12 ",
        ] {
            assert_eq!(avec_pid(Some(refuse)), None, "{refuse:?}");
        }
        assert_eq!(avec_pid(Some(&"9".repeat(5000))), None);
        assert_eq!(avec_pid(Some("9999999")), Some(9_999_999));
        assert_eq!(avec_pid(Some("2")), Some(2));
    }

    fn avec_cwd(brut: &str) -> Option<String> {
        let t = tmp();
        filtrer(
            &corps(json!({"hook_event_name": "Stop", "session_id": "s1", "cwd": brut})),
            None,
            0.0,
            t.path(),
        )
        .unwrap()
        .cwd
    }

    #[test]
    fn filtrer_cwd() {
        assert_eq!(avec_cwd("/a\u{0}b"), None);
        assert_eq!(avec_cwd("relatif/x"), None);
        assert_eq!(avec_cwd(""), None);
        assert_eq!(avec_cwd("/Users/x/p").as_deref(), Some("/Users/x/p"));
    }

    fn avec_agent(brut: &str) -> Option<String> {
        let t = tmp();
        filtrer(
            &corps(json!({"hook_event_name": "Stop", "session_id": "s1", "agent_id": brut})),
            None,
            0.0,
            t.path(),
        )
        .unwrap()
        .agent_id
    }

    #[test]
    fn filtrer_agent_id() {
        assert_eq!(avec_agent(""), None);
        assert_eq!(
            avec_agent("a19de871c0abcdef1").as_deref(),
            Some("a19de871c0abcdef1")
        );
        assert_eq!(avec_agent("../etc"), None);
        assert_eq!(avec_agent("a\n"), None);
        assert_eq!(avec_agent(&"a".repeat(129)), None);
        assert!(avec_agent(&"a".repeat(128)).is_some());
    }

    #[test]
    fn identifiant_ascii_seulement() {
        assert!(!identifiant_valide("é"));
        assert!(!identifiant_valide("a b"));
        assert!(identifiant_valide("A_b-9"));
    }

    // --- transcript_autorise ---

    #[test]
    fn transcript_autorise_cas_de_base() {
        let t = tmp();
        let racine = t.path().join("projects");
        fs::create_dir_all(racine.join("p")).unwrap();
        let s = |p: &Path| p.to_str().unwrap().to_owned();
        assert!(transcript_autorise(&s(&racine.join("p/s1.jsonl")), &racine));
        assert!(!transcript_autorise(&s(&racine.join("p/s1.txt")), &racine));
        assert!(!transcript_autorise(
            &s(&t.path().join("ailleurs.jsonl")),
            &racine
        ));
        assert!(!transcript_autorise(
            &s(&racine.join("../ailleurs.jsonl")),
            &racine
        ));
        assert!(!transcript_autorise("relatif/s1.jsonl", &racine));
        // le suffixe de Python : `.jsonl` seul n'a pas de suffixe
        assert!(!transcript_autorise(&s(&racine.join("p/.jsonl")), &racine));
        assert!(!transcript_autorise(&s(&racine.join("p/..")), &racine));
    }

    #[test]
    fn transcript_autorise_faux_sur_nul() {
        let t = tmp();
        let racine = t.path().join("projects");
        fs::create_dir_all(&racine).unwrap();
        assert!(!transcript_autorise("/a\u{0}b.jsonl", &racine));
    }

    #[test]
    fn transcript_autorise_faux_sur_boucle_symlink() {
        let t = tmp();
        let racine = t.path().join("projects");
        fs::create_dir_all(racine.join("p")).unwrap();
        let boucle = racine.join("p/boucle.jsonl");
        symlink(&boucle, &boucle).unwrap();
        assert!(!transcript_autorise(boucle.to_str().unwrap(), &racine));
    }

    #[test]
    fn transcript_autorise_accepte_un_fichier_pas_encore_ecrit() {
        let t = tmp();
        let racine = t.path().join("projects");
        fs::create_dir_all(racine.join("p")).unwrap();
        let p = racine.join("p/pas-encore-ecrit.jsonl");
        assert!(transcript_autorise(p.to_str().unwrap(), &racine));
    }

    #[test]
    fn transcript_autorise_faux_sur_boucle_dans_un_repertoire() {
        let t = tmp();
        let racine = t.path().join("projects");
        fs::create_dir_all(&racine).unwrap();
        let boucle = racine.join("boucle");
        symlink(&boucle, &boucle).unwrap();
        let p = boucle.join("x.jsonl");
        assert!(!transcript_autorise(p.to_str().unwrap(), &racine));
    }

    #[test]
    fn transcript_autorise_suit_les_liens_vers_l_exterieur() {
        // un lien dans la racine qui pointe hors de la racine est refusé
        let t = tmp();
        let racine = t.path().join("projects");
        fs::create_dir_all(&racine).unwrap();
        let dehors = t.path().join("dehors");
        fs::create_dir_all(&dehors).unwrap();
        symlink(&dehors, racine.join("lien")).unwrap();
        let p = racine.join("lien/x.jsonl");
        assert!(!transcript_autorise(p.to_str().unwrap(), &racine));
    }

    #[test]
    fn filtrer_efface_un_transcript_hors_racine() {
        let t = tmp();
        let ev = filtrer(
            &corps(json!({"hook_event_name": "Stop", "session_id": "s1", "transcript_path": "/etc/passwd.jsonl"})),
            None,
            0.0,
            t.path(),
        )
        .unwrap();
        assert_eq!(ev.transcript_path, None);
    }

    // --- entier_python ---

    #[test]
    fn entier_python_comme_int() {
        assert_eq!(entier_python("12"), Some(12));
        assert_eq!(entier_python("+12"), Some(12));
        assert_eq!(entier_python("-12"), Some(-12));
        assert_eq!(entier_python("007"), Some(7));
        assert_eq!(entier_python("1_000"), Some(1000));
        for non in [
            "", "-", "abc", "1.5", "1__0", "_1", "1_", "1 2", "0x10", "٣",
        ] {
            assert_eq!(entier_python(non), None, "{non:?}");
        }
        assert!(entier_python(&"9".repeat(4301)).is_none());
        assert!(entier_python(&"9".repeat(4000)).unwrap() > TAILLE_MAX as i128);
    }
}

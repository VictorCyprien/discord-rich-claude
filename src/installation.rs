//! Installation : hooks dans `~/.claude/settings.json`, LaunchAgent. Voir la spec, section 8.
//!
//! Nos hooks se reconnaissent au marqueur dans leur commande. Les hooks d'autres
//! outils ne sont jamais touchés, même dans un groupe mixte. Toute forme qu'on ne
//! sait pas modifier sans risque (valeur d'événement qui n'est pas une liste,
//! groupe qui n'est pas un objet ou dont les hooks ne sont pas une liste) est
//! laissée intacte plutôt que devinée ; quand on doit ajouter nos hooks dans une
//! forme pareille, on refuse avec une erreur claire plutôt que de l'abîmer.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::NaiveDateTime;
use serde_json::{Map, Value, json};
use similar::TextDiff;

use crate::chemins::LABEL;

pub const MARQUEUR: &str = "discord-rich-claude";
pub const EVENEMENTS_HOOKS: [&str; 8] = [
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "Stop",
    "SubagentStart",
    "SubagentStop",
    "SessionEnd",
];

fn est_a_nous(hook: &Value) -> bool {
    let Some(objet) = hook.as_object() else {
        return false;
    };
    match objet.get("command") {
        None => false,
        Some(Value::String(c)) => c.contains(MARQUEUR),
        Some(autre) => autre.to_string().contains(MARQUEUR),
    }
}

/// Groupes sans nos hooks, et « on en a retiré au moins un ».
fn sans_les_notres(groupes: &[Value]) -> (Vec<Value>, bool) {
    let mut resultat = Vec::with_capacity(groupes.len());
    let mut retire = false;
    for groupe in groupes {
        let hooks = match groupe.as_object() {
            Some(g) => match g.get("hooks") {
                None => Some(&[][..]),
                Some(Value::Array(h)) => Some(h.as_slice()),
                Some(_) => None,
            },
            None => None,
        };
        let (Some(objet), Some(hooks)) = (groupe.as_object(), hooks) else {
            resultat.push(groupe.clone());
            continue;
        };
        let gardes: Vec<Value> = hooks.iter().filter(|h| !est_a_nous(h)).cloned().collect();
        if gardes.len() == hooks.len() {
            resultat.push(groupe.clone());
            continue;
        }
        retire = true;
        if gardes.is_empty() {
            continue;
        }
        let mut copie: Map<String, Value> = objet.clone();
        copie.insert("hooks".into(), Value::Array(gardes));
        resultat.push(Value::Object(copie));
    }
    (resultat, retire)
}

pub fn ajouter_hooks(settings: &Value, commande: &str) -> Result<Value, String> {
    let Some(objet) = settings.as_object() else {
        return Err("settings doit être un objet".into());
    };
    let mut nouveau = objet.clone();
    if let Some(hooks) = nouveau.get("hooks") {
        let Some(brut) = hooks.as_object() else {
            return Err("settings['hooks'] doit être un objet".into());
        };
        for ev in EVENEMENTS_HOOKS {
            if brut.get(ev).is_some_and(|v| !v.is_array()) {
                return Err(format!("settings['hooks']['{ev}'] doit être une liste"));
            }
        }
    }
    // `setdefault` de Python : "hooks" garde sa place s'il existait, sinon va en fin.
    let Some(hooks) = nouveau
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
    else {
        return Err("settings['hooks'] doit être un objet".into());
    };
    for ev in EVENEMENTS_HOOKS {
        let existants = hooks.get(ev).and_then(Value::as_array).map(Vec::as_slice);
        let (mut groupes, _) = sans_les_notres(existants.unwrap_or_default());
        let hook = json!({"type": "command", "command": commande, "timeout": 2});
        let groupe = if ev == "PreToolUse" {
            json!({"matcher": "*", "hooks": [hook]})
        } else {
            json!({"hooks": [hook]})
        };
        groupes.push(groupe);
        hooks.insert(ev.into(), Value::Array(groupes));
    }
    Ok(Value::Object(nouveau))
}

pub fn retirer_hooks(settings: &Value) -> Result<Value, String> {
    let Some(objet) = settings.as_object() else {
        return Err("settings doit être un objet".into());
    };
    let mut nouveau = objet.clone();
    let Some(valeur) = nouveau.get_mut("hooks") else {
        return Ok(Value::Object(nouveau));
    };
    let Some(hooks) = valeur.as_object_mut() else {
        return Err("settings['hooks'] doit être un objet".into());
    };
    let mut a_supprime_un_evenement = false;
    let evenements: Vec<String> = hooks.keys().cloned().collect();
    for ev in evenements {
        let Some(Value::Array(valeur)) = hooks.get(&ev) else {
            continue;
        };
        let (groupes, retire) = sans_les_notres(valeur);
        if retire && groupes.is_empty() {
            hooks.shift_remove(&ev);
            a_supprime_un_evenement = true;
        } else {
            hooks.insert(ev, Value::Array(groupes));
        }
    }
    if a_supprime_un_evenement && hooks.is_empty() {
        nouveau.shift_remove("hooks");
    }
    Ok(Value::Object(nouveau))
}

/// Issue de `modifier_settings`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resultat {
    Inchange,
    Refuse,
    Modifie,
}

impl fmt::Display for Resultat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Inchange => "inchangé",
            Self::Refuse => "refusé",
            Self::Modifie => "modifié",
        })
    }
}

fn lire_octets(chemin: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::read(chemin) {
        Ok(o) => Ok(Some(o)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!(
            "lecture de {} impossible ({:?})",
            chemin.display(),
            e.kind()
        )),
    }
}

pub(crate) fn erreur_io(action: &str, e: &io::Error) -> String {
    format!("{action} impossible ({:?})", e.kind())
}

/// Ouvre un fichier neuf en écriture, exclusivement, directement au `mode`
/// voulu (l'umask ne peut qu'enlever des droits, jamais en ajouter).
fn ouvrir_exclusif(chemin: &Path, mode: u32) -> io::Result<fs::File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(chemin)
}

/// Sauvegarde exclusive : jamais d'écrasement d'une sauvegarde existante.
///
/// Créée directement au mode voulu (`.mode()`), puis `set_permissions` avant
/// toute écriture : l'umask ne peut jamais la laisser plus large ni plus étroite
/// que l'original.
fn sauvegarder(chemin: &Path, contenu: &[u8], mode: u32, horloge: NaiveDateTime) -> io::Result<()> {
    let nom = chemin
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let base = format!("{nom}.bak-{}", horloge.format("%Y%m%d-%H%M%S"));
    let mut suffixe = 0u32;
    loop {
        let candidat = if suffixe == 0 {
            base.clone()
        } else {
            format!("{base}-{suffixe}")
        };
        let cible = chemin.with_file_name(candidat);
        match ouvrir_exclusif(&cible, mode) {
            Ok(mut f) => {
                f.set_permissions(fs::Permissions::from_mode(mode))?;
                f.write_all(contenu)?;
                return Ok(());
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => suffixe += 1,
            Err(e) => return Err(e),
        }
    }
}

/// Crée un fichier temporaire frère de `cible`, exclusif, au mode voulu.
fn creer_temporaire(cible: &Path, prefixe: &str, mode: u32) -> io::Result<(fs::File, PathBuf)> {
    let dossier = cible.parent().unwrap_or(Path::new("."));
    let pid = std::process::id();
    let graine = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    for essai in 0u32..1000 {
        let alea = graine
            .wrapping_mul(2_654_435_761)
            .wrapping_add(essai.wrapping_mul(40_503));
        let chemin = dossier.join(format!("{prefixe}{pid:x}{alea:08x}.tmp"));
        match ouvrir_exclusif(&chemin, mode) {
            Ok(f) => return Ok((f, chemin)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "nom temporaire introuvable",
    ))
}

/// Écriture atomique : temporaire frère au `mode` voulu, `sync_all`, puis `renommer`.
/// Le temporaire est supprimé en cas d'échec.
pub(crate) fn ecrire_atomique(
    cible: &Path,
    contenu: &[u8],
    mode: u32,
    prefixe: &str,
    renommer: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    let (mut f, temporaire) = creer_temporaire(cible, prefixe, mode)?;
    let resultat = (|| {
        f.set_permissions(fs::Permissions::from_mode(mode))?;
        f.write_all(contenu)?;
        f.flush()?;
        f.sync_all()?;
        renommer(&temporaire, cible)
    })();
    if resultat.is_err() {
        let _ = fs::remove_file(&temporaire);
    }
    resultat
}

pub(crate) fn ecrire_fichier_atomique(cible: &Path, contenu: &[u8], mode: u32) -> io::Result<()> {
    let nom = cible
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    ecrire_atomique(cible, contenu, mode, &format!(".{nom}."), |de, vers| {
        fs::rename(de, vers)
    })
}

fn joli(valeur: &Value) -> Result<String, String> {
    serde_json::to_string_pretty(valeur).map_err(|_| "sérialisation JSON impossible".to_string())
}

pub fn modifier_settings(
    chemin: &Path,
    transformer: impl Fn(&Value) -> Result<Value, String>,
    confirmer: impl FnOnce(&str) -> bool,
    horloge: impl Fn() -> NaiveDateTime,
) -> Result<Resultat, String> {
    let avant_octets = lire_octets(chemin)?;
    let ancien: Value = match &avant_octets {
        Some(o) => serde_json::from_slice(o)
            .map_err(|_| format!("{} n'est pas du JSON valide", chemin.display()))?,
        None => json!({}),
    };
    if !ancien.is_object() {
        return Err(format!(
            "{} ne contient pas un objet JSON",
            chemin.display()
        ));
    }
    let nouveau = transformer(&ancien)?;
    if nouveau == ancien {
        return Ok(Resultat::Inchange);
    }
    let avant_texte = joli(&ancien)?;
    let apres_texte = joli(&nouveau)?;
    let diff = TextDiff::from_lines(&avant_texte, &apres_texte)
        .unified_diff()
        .context_radius(3)
        // Le fichier écrit se termine par un `\n` : la ligne « No newline at end of
        // file » serait trompeuse juste avant la confirmation.
        .missing_newline_hint(false)
        .header("avant", "après")
        .to_string();
    if !confirmer(diff.trim_end_matches('\n')) {
        return Ok(Resultat::Refuse);
    }

    // La confirmation peut prendre du temps (interaction utilisateur) ; Claude
    // Code réécrit lui-même settings.json entre-temps. On relit juste avant
    // d'écrire et on abandonne plutôt que d'écraser une modification perdue.
    if lire_octets(chemin)? != avant_octets {
        return Err("settings.json a changé pendant la confirmation : relance la commande".into());
    }

    let mode = if avant_octets.is_some() {
        fs::metadata(chemin)
            .map_err(|e| erreur_io("lecture des droits", &e))?
            .permissions()
            .mode()
            & 0o7777
    } else {
        0o600
    };

    if let Some(parent) = chemin.parent() {
        fs::create_dir_all(parent).map_err(|e| erreur_io("création du dossier", &e))?;
    }
    if let Some(octets) = &avant_octets {
        sauvegarder(chemin, octets, mode, horloge()).map_err(|e| erreur_io("sauvegarde", &e))?;
    }
    let payload = format!("{apres_texte}\n");
    ecrire_fichier_atomique(chemin, payload.as_bytes(), mode)
        .map_err(|e| erreur_io("écriture", &e))?;
    Ok(Resultat::Modifie)
}

// --- LaunchAgent ---

/// Sortie d'un processus externe (`launchctl`, `codesign`).
pub struct Sortie {
    pub code: i32,
    pub stderr: Vec<u8>,
}

/// Chemins absolus des outils système : jamais cherchés dans le `PATH`.
pub const LAUNCHCTL: &str = "/bin/launchctl";
pub const CODESIGN: &str = "/usr/bin/codesign";

pub type Executeur<'a> = &'a mut dyn FnMut(&[&str]) -> io::Result<Sortie>;

/// Exécuteur de production : lance la commande et capture sa sortie.
pub fn executeur_reel(commande: &[&str]) -> io::Result<Sortie> {
    let Some((programme, arguments)) = commande.split_first() else {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "commande vide"));
    };
    let sortie = std::process::Command::new(programme)
        .args(arguments)
        .output()?;
    Ok(Sortie {
        code: sortie.status.code().unwrap_or(-1),
        stderr: sortie.stderr,
    })
}

#[derive(Debug)]
pub enum ErreurLaunchctl {
    Io(io::Error),
    Echec { code: i32, stderr: String },
}

impl fmt::Display for ErreurLaunchctl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "launchctl inutilisable ({:?})", e.kind()),
            Self::Echec { code, stderr } => {
                write!(f, "launchctl a échoué (code {code}) : {stderr}")
            }
        }
    }
}

impl std::error::Error for ErreurLaunchctl {}

impl From<io::Error> for ErreurLaunchctl {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

pub(crate) fn echapper_xml(texte: &str) -> String {
    texte
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub(crate) const ENTETE_PLIST: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n<dict>\n";

pub(crate) fn plist_chaine(cle: &str, valeur: &str) -> String {
    format!(
        "\t<key>{}</key>\n\t<string>{}</string>\n",
        echapper_xml(cle),
        echapper_xml(valeur)
    )
}

pub fn generer_plist(executable: &Path, dossier_logs: &Path) -> Vec<u8> {
    let chemin_log = |nom: &str| dossier_logs.join(nom).to_string_lossy().into_owned();
    let mut xml = String::from(ENTETE_PLIST);
    xml += &plist_chaine("Label", LABEL);
    xml += "\t<key>ProgramArguments</key>\n\t<array>\n";
    xml += &format!(
        "\t\t<string>{}</string>\n",
        echapper_xml(&executable.to_string_lossy())
    );
    xml += "\t\t<string>run</string>\n\t</array>\n";
    xml += "\t<key>RunAtLoad</key>\n\t<true/>\n";
    xml += &plist_chaine("StandardErrorPath", &chemin_log("launchd.err.log"));
    xml += &plist_chaine("StandardOutPath", &chemin_log("launchd.out.log"));
    xml += "</dict>\n</plist>\n";
    xml.into_bytes()
}

const REESSAIS_BOOTSTRAP: u32 = 3;
const DELAI_REESSAI_BOOTSTRAP: Duration = Duration::from_millis(500);
const CODES_BOOTSTRAP_TRANSITOIRES: [i32; 2] = [5, 37];

pub fn installer_launch_agent(
    chemin_plist: &Path,
    contenu: &[u8],
    uid: u32,
    executer: Executeur,
    dormir: &mut dyn FnMut(Duration),
) -> Result<(), ErreurLaunchctl> {
    let domaine = format!("gui/{uid}");
    // Le plist est écrit avant tout appel à launchctl : un échec d'écriture (droits,
    // disque plein) ne doit jamais décharger un daemon qui tournait déjà.
    if let Some(parent) = chemin_plist.parent() {
        fs::create_dir_all(parent)?;
    }
    ecrire_fichier_atomique(chemin_plist, contenu, 0o600)?;
    // Décharge une version précédente ; échoue sans conséquence si rien n'est chargé.
    let _ = executer(&[LAUNCHCTL, "bootout", &format!("{domaine}/{LABEL}")]);
    // `bootstrap` juste après un `bootout` échoue par intermittence (code 5 ou 37)
    // tant que l'ancienne instance finit de se décharger : quelques essais suffisent.
    let chemin = chemin_plist.to_string_lossy().into_owned();
    // Les essais intermédiaires ne rendent la main que sur un succès ou un échec définitif ;
    // le dernier essai rend toujours son résultat.
    for _ in 1..REESSAIS_BOOTSTRAP {
        let sortie = executer(&[LAUNCHCTL, "bootstrap", &domaine, &chemin])?;
        if sortie.code == 0 {
            return Ok(());
        }
        if !CODES_BOOTSTRAP_TRANSITOIRES.contains(&sortie.code) {
            return Err(echec_launchctl(&sortie));
        }
        dormir(DELAI_REESSAI_BOOTSTRAP);
    }
    let sortie = executer(&[LAUNCHCTL, "bootstrap", &domaine, &chemin])?;
    if sortie.code == 0 {
        Ok(())
    } else {
        Err(echec_launchctl(&sortie))
    }
}

fn echec_launchctl(sortie: &Sortie) -> ErreurLaunchctl {
    ErreurLaunchctl::Echec {
        code: sortie.code,
        stderr: String::from_utf8_lossy(&sortie.stderr).trim().to_string(),
    }
}

pub fn desinstaller_launch_agent(
    chemin_plist: &Path,
    uid: u32,
    executer: Executeur,
) -> io::Result<()> {
    let _ = executer(&[LAUNCHCTL, "bootout", &format!("gui/{uid}/{LABEL}")]);
    match fs::remove_file(chemin_plist) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chemins::{COMMANDE_HOOK, LABEL};
    use crate::serveur::GardeUmask;
    use chrono::NaiveDate;
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Mutex;

    /// Le masque de création est global au processus : les tests qui le posent
    /// ne se chevauchent pas.
    static VERROU_UMASK: Mutex<()> = Mutex::new(());

    #[test]
    fn launchctl_et_codesign_sont_des_chemins_absolus() {
        // Jamais une recherche dans le `PATH` : un binaire du même nom posé ailleurs ne
        // doit pas être lancé à la place de celui du système.
        assert!(Path::new(LAUNCHCTL).is_absolute());
        assert!(Path::new(CODESIGN).is_absolute());
    }

    fn tiers() -> Value {
        json!({"type": "command", "command": "afplay /System/Library/Sounds/Glass.aiff"})
    }

    fn settings_tiers() -> Value {
        json!({
            "model": "opus",
            "hooks": {
                "Stop": [{"hooks": [tiers()]}],
                "PostToolUse": [{"matcher": "Edit", "hooks": [tiers()]}],
            },
        })
    }

    fn nos_hooks(settings: &Value, ev: &str) -> Vec<Value> {
        settings["hooks"][ev]
            .as_array()
            .map(|v| v.as_slice())
            .unwrap_or_default()
            .iter()
            .flat_map(|g| g["hooks"].as_array().unwrap().clone())
            .filter(|h| {
                h["command"]
                    .as_str()
                    .is_some_and(|c| c.contains("discord-rich-claude"))
            })
            .collect()
    }

    fn notre_hook() -> Value {
        json!({"type": "command", "command": COMMANDE_HOOK, "timeout": 2})
    }

    fn horloge() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 9, 18)
            .unwrap()
            .and_hms_opt(14, 30, 5)
            .unwrap()
    }

    fn ajouter(s: &Value) -> Value {
        ajouter_hooks(s, COMMANDE_HOOK).unwrap()
    }

    #[test]
    fn test_ajouter_pose_les_8_evenements() {
        let s = ajouter(&json!({}));
        let cles: std::collections::HashSet<_> =
            s["hooks"].as_object().unwrap().keys().cloned().collect();
        let attendues: std::collections::HashSet<_> =
            EVENEMENTS_HOOKS.iter().map(|e| e.to_string()).collect();
        assert_eq!(cles, attendues);
        assert_eq!(EVENEMENTS_HOOKS.len(), 8);
        for ev in EVENEMENTS_HOOKS {
            assert_eq!(nos_hooks(&s, ev), vec![notre_hook()]);
        }
        assert_eq!(s["hooks"]["PreToolUse"][0]["matcher"], "*");
    }

    #[test]
    fn test_ajouter_conserve_les_hooks_tiers_et_ne_modifie_pas_l_entree() {
        let avant = settings_tiers();
        let copie = avant.clone();
        let s = ajouter(&avant);
        assert_eq!(avant, copie);
        assert_eq!(s["model"], "opus");
        assert_eq!(s["hooks"]["Stop"][0], json!({"hooks": [tiers()]}));
        assert_eq!(
            s["hooks"]["PostToolUse"],
            json!([{"matcher": "Edit", "hooks": [tiers()]}])
        );
    }

    #[test]
    fn ajouter_garde_l_ordre_des_cles() {
        let s: Value =
            serde_json::from_str(r#"{"zeta":1,"model":"x","alpha":{"b":1,"a":2}}"#).unwrap();
        let n = ajouter_hooks(&s, COMMANDE_HOOK).unwrap();
        let cles: Vec<_> = n.as_object().unwrap().keys().cloned().collect();
        assert_eq!(cles, ["zeta", "model", "alpha", "hooks"]);
        assert_eq!(
            n["alpha"].as_object().unwrap().keys().collect::<Vec<_>>(),
            ["b", "a"]
        );
    }

    #[test]
    fn ajouter_garde_la_place_d_un_hooks_existant() {
        let s: Value = serde_json::from_str(r#"{"a":1,"hooks":{"Stop":[],"X":[]},"z":2}"#).unwrap();
        let n = ajouter_hooks(&s, COMMANDE_HOOK).unwrap();
        let cles: Vec<_> = n.as_object().unwrap().keys().cloned().collect();
        assert_eq!(cles, ["a", "hooks", "z"]);
        let evenements: Vec<_> = n["hooks"].as_object().unwrap().keys().cloned().collect();
        assert_eq!(&evenements[..2], ["Stop", "X"]);
    }

    #[test]
    fn test_ajouter_est_idempotent() {
        let une_fois = ajouter(&settings_tiers());
        assert_eq!(ajouter(&une_fois), une_fois);
    }

    #[test]
    fn test_retirer_ne_retire_que_les_notres() {
        assert_eq!(
            retirer_hooks(&ajouter(&settings_tiers())).unwrap(),
            settings_tiers()
        );
    }

    #[test]
    fn test_retirer_d_un_groupe_mixte_garde_le_hook_tiers() {
        let s = json!({"hooks": {"Stop": [{"hooks": [tiers(), {"type": "command", "command": COMMANDE_HOOK}]}]}});
        assert_eq!(
            retirer_hooks(&s).unwrap(),
            json!({"hooks": {"Stop": [{"hooks": [tiers()]}]}})
        );
    }

    #[test]
    fn test_retirer_sans_hooks_ne_change_rien() {
        assert_eq!(
            retirer_hooks(&json!({"model": "opus"})).unwrap(),
            json!({"model": "opus"})
        );
        assert_eq!(
            retirer_hooks(&json!({"hooks": {"Stop": []}})).unwrap(),
            json!({"hooks": {"Stop": []}})
        );
    }

    fn ecrire(chemin: &Path, valeur: &Value) {
        fs::write(chemin, serde_json::to_string(valeur).unwrap()).unwrap();
    }

    fn lire(chemin: &Path) -> Value {
        serde_json::from_slice(&fs::read(chemin).unwrap()).unwrap()
    }

    fn noms(dossier: &Path) -> std::collections::BTreeSet<String> {
        fs::read_dir(dossier)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect()
    }

    fn mode(chemin: &Path) -> u32 {
        fs::metadata(chemin).unwrap().permissions().mode() & 0o7777
    }

    fn ajouter_t(s: &Value) -> Result<Value, String> {
        ajouter_hooks(s, COMMANDE_HOOK)
    }

    #[test]
    fn test_modifier_settings_sauvegarde_puis_ecrit() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("settings.json");
        ecrire(&f, &settings_tiers());
        let mut diffs = vec![];
        let res = modifier_settings(
            &f,
            ajouter_t,
            |diff| {
                diffs.push(diff.to_string());
                true
            },
            horloge,
        );
        assert_eq!(res, Ok(Resultat::Modifie));
        assert_eq!(lire(&f), ajouter(&settings_tiers()));
        let sauvegarde = d.path().join("settings.json.bak-20260918-143005");
        assert_eq!(lire(&sauvegarde), settings_tiers());
        assert!(diffs[0].contains("discord-rich-claude"));
        assert!(diffs[0].starts_with("--- avant\n+++ après"));
        assert!(fs::read_to_string(&f).unwrap().ends_with("}\n"));
    }

    #[test]
    fn test_modifier_settings_refuse_n_ecrit_rien() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("settings.json");
        ecrire(&f, &settings_tiers());
        assert_eq!(
            modifier_settings(&f, ajouter_t, |_| false, horloge),
            Ok(Resultat::Refuse)
        );
        assert_eq!(lire(&f), settings_tiers());
        assert_eq!(noms(d.path()), ["settings.json".to_string()].into());
    }

    #[test]
    fn diff_sans_indication_de_fin_de_ligne() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("settings.json");
        std::fs::write(&f, "{}").unwrap();
        let mut vu = String::new();
        modifier_settings(
            &f,
            ajouter_t,
            |d| {
                vu = d.to_string();
                false
            },
            horloge,
        )
        .unwrap();
        assert!(!vu.is_empty());
        assert!(!vu.contains("No newline"), "{vu}");
    }

    #[test]
    fn test_modifier_settings_inchange() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("settings.json");
        ecrire(&f, &json!({"model": "opus"}));
        assert_eq!(
            modifier_settings(&f, retirer_hooks, |_| true, horloge),
            Ok(Resultat::Inchange)
        );
        assert_eq!(noms(d.path()), ["settings.json".to_string()].into());
    }

    #[test]
    fn test_modifier_settings_fichier_absent() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("settings.json");
        assert_eq!(
            modifier_settings(&f, ajouter_t, |_| true, horloge),
            Ok(Resultat::Modifie)
        );
        assert_eq!(lire(&f), ajouter(&json!({})));
        assert_eq!(mode(&f), 0o600);
        assert_eq!(noms(d.path()), ["settings.json".to_string()].into());
    }

    #[test]
    fn test_modifier_settings_invalide_leve_sans_ecrire() {
        for contenu in ["{pas du json", "[1, 2]"] {
            let d = tempfile::tempdir().unwrap();
            let f = d.path().join("settings.json");
            fs::write(&f, contenu).unwrap();
            assert!(modifier_settings(&f, ajouter_t, |_| true, horloge).is_err());
            assert_eq!(fs::read_to_string(&f).unwrap(), contenu);
            assert_eq!(noms(d.path()), ["settings.json".to_string()].into());
        }
    }

    #[test]
    fn test_modifier_settings_detecte_changement_concurrent() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("settings.json");
        ecrire(&f, &settings_tiers());
        let f2 = f.clone();
        let res = modifier_settings(
            &f,
            ajouter_t,
            |_| {
                ecrire(&f2, &json!({"model": "sonnet"}));
                true
            },
            horloge,
        );
        assert_eq!(
            res,
            Err("settings.json a changé pendant la confirmation : relance la commande".to_string())
        );
        assert_eq!(lire(&f), json!({"model": "sonnet"}));
        assert_eq!(noms(d.path()), ["settings.json".to_string()].into());
    }

    #[test]
    fn test_modifier_settings_detecte_apparition_du_fichier() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("settings.json");
        let f2 = f.clone();
        let res = modifier_settings(
            &f,
            ajouter_t,
            |_| {
                ecrire(&f2, &json!({"model": "sonnet"}));
                true
            },
            horloge,
        );
        assert!(res.is_err());
        assert_eq!(lire(&f), json!({"model": "sonnet"}));
        assert_eq!(noms(d.path()), ["settings.json".to_string()].into());
    }

    #[test]
    fn test_modifier_settings_conserve_les_droits() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("settings.json");
        ecrire(&f, &settings_tiers());
        fs::set_permissions(&f, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            modifier_settings(&f, ajouter_t, |_| true, horloge),
            Ok(Resultat::Modifie)
        );
        assert_eq!(mode(&f), 0o600);
        assert_eq!(
            mode(&d.path().join("settings.json.bak-20260918-143005")),
            0o600
        );
    }

    #[test]
    fn modifier_settings_droits_gardes_meme_sous_umask_000_et_077() {
        let _v = VERROU_UMASK.lock().unwrap_or_else(|e| e.into_inner());
        for (masque, droits) in [(0o000, 0o600), (0o077, 0o644), (0o000, 0o640)] {
            let _u = GardeUmask::poser(masque);
            let d = tempfile::tempdir().unwrap();
            let f = d.path().join("settings.json");
            ecrire(&f, &settings_tiers());
            fs::set_permissions(&f, fs::Permissions::from_mode(droits)).unwrap();
            assert_eq!(
                modifier_settings(&f, ajouter_t, |_| true, horloge),
                Ok(Resultat::Modifie)
            );
            assert_eq!(mode(&f), droits, "umask {masque:o}");
            assert_eq!(
                mode(&d.path().join("settings.json.bak-20260918-143005")),
                droits,
                "umask {masque:o}"
            );
        }
    }

    #[test]
    fn test_modifier_settings_sauvegarde_deja_au_bon_mode_des_la_creation() {
        // Garde F7 : sous umask 000, un fichier créé sans `.mode()` naîtrait en
        // 0666. On lit le mode juste après l'ouverture, sans `set_permissions`.
        let _v = VERROU_UMASK.lock().unwrap_or_else(|e| e.into_inner());
        let _u = GardeUmask::poser(0o000);
        let d = tempfile::tempdir().unwrap();
        // Sauvegarde : ouverture exclusive partagée par `sauvegarder`.
        let chemin = d.path().join("settings.json.bak");
        let f = ouvrir_exclusif(&chemin, 0o600).unwrap();
        assert_eq!(f.metadata().unwrap().permissions().mode() & 0o7777, 0o600);
        // Temporaire de réécriture.
        let (f, tmp) =
            creer_temporaire(&d.path().join("settings.json"), ".settings.json.", 0o600).unwrap();
        assert_eq!(f.metadata().unwrap().permissions().mode() & 0o7777, 0o600);
        assert_eq!(mode(&tmp), 0o600);
    }

    #[test]
    fn test_modifier_settings_deux_sauvegardes_meme_seconde_ne_s_ecrasent_pas() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("settings.json");
        ecrire(&f, &settings_tiers());
        assert_eq!(
            modifier_settings(&f, ajouter_t, |_| true, horloge),
            Ok(Resultat::Modifie)
        );
        let apres_premier = lire(&f);
        let b0 = d.path().join("settings.json.bak-20260918-143005");
        let contenu_b0 = fs::read_to_string(&b0).unwrap();
        assert_eq!(
            modifier_settings(&f, retirer_hooks, |_| true, horloge),
            Ok(Resultat::Modifie)
        );
        let b1 = d.path().join("settings.json.bak-20260918-143005-1");
        assert!(b1.exists());
        assert_eq!(fs::read_to_string(&b0).unwrap(), contenu_b0);
        assert_eq!(lire(&b1), apres_premier);
    }

    #[test]
    fn test_modifier_settings_echec_a_l_ecriture_ne_laisse_pas_de_fichier_temporaire() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("settings.json");
        fs::write(&f, "initial").unwrap();
        let res = ecrire_atomique(&f, b"nouveau", 0o600, ".settings.json.", |_, _| {
            Err(io::Error::other("échec simulé"))
        });
        assert!(res.is_err());
        assert_eq!(fs::read_to_string(&f).unwrap(), "initial");
        assert_eq!(noms(d.path()), ["settings.json".to_string()].into());
    }

    #[test]
    fn test_modifier_settings_transformer_non_serialisable_ne_laisse_rien() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("settings.json");
        let contenu_initial = br#"{"model": "opus"}"#;
        fs::write(&f, contenu_initial).unwrap();
        let res = modifier_settings(
            &f,
            |_| Err("objet non sérialisable".to_string()),
            |_| true,
            horloge,
        );
        assert!(res.is_err());
        assert_eq!(fs::read(&f).unwrap(), contenu_initial);
        assert_eq!(noms(d.path()), ["settings.json".to_string()].into());
    }

    #[test]
    fn test_retirer_laisse_intacte_une_valeur_d_evenement_qui_est_un_objet() {
        let s = json!({"hooks": {"Stop": {"matcher": "x", "hooks": [tiers()]}}});
        assert_eq!(retirer_hooks(&s).unwrap(), s);
    }

    #[test]
    fn test_retirer_laisse_intacte_une_valeur_d_evenement_qui_est_une_chaine() {
        let s = json!({"hooks": {"Stop": "abc"}});
        assert_eq!(retirer_hooks(&s).unwrap(), s);
    }

    #[test]
    fn test_retirer_laisse_intact_un_groupe_qui_n_est_pas_un_objet() {
        let s = json!({"hooks": {"Stop": ["pas-un-objet"]}});
        assert_eq!(retirer_hooks(&s).unwrap(), s);
    }

    #[test]
    fn test_retirer_laisse_intact_un_groupe_dont_les_hooks_ne_sont_pas_une_liste() {
        let s = json!({"hooks": {"Stop": [{"hooks": null}]}});
        assert_eq!(retirer_hooks(&s).unwrap(), s);
    }

    #[test]
    fn test_retirer_garde_un_objet_hooks_deja_vide() {
        assert_eq!(
            retirer_hooks(&json!({"hooks": {}})).unwrap(),
            json!({"hooks": {}})
        );
    }

    #[test]
    fn test_retirer_leve_si_hooks_n_est_pas_un_objet() {
        assert!(retirer_hooks(&json!({"hooks": "abc"})).is_err());
        assert!(retirer_hooks(&json!({"hooks": null})).is_err());
    }

    #[test]
    fn test_ajouter_leve_si_hooks_n_est_pas_un_objet() {
        assert!(ajouter_t(&json!({"hooks": "abc"})).is_err());
        assert!(ajouter_t(&json!({"hooks": null})).is_err());
    }

    #[test]
    fn test_ajouter_leve_si_un_evenement_n_est_pas_une_liste_et_nomme_l_evenement() {
        let e = ajouter_t(&json!({"hooks": {"Stop": "abc"}})).unwrap_err();
        assert!(e.contains("Stop"));
    }

    #[test]
    fn test_ajouter_preserve_un_groupe_malforme_sans_planter() {
        let s = json!({"hooks": {"Stop": [{"hooks": null}]}});
        let r = ajouter(&s);
        let groupes = r["hooks"]["Stop"].as_array().unwrap();
        assert!(groupes.contains(&json!({"hooks": null})));
        assert!(groupes.contains(&json!({"hooks": [notre_hook()]})));
    }

    // --- LaunchAgent ---

    fn texte(octets: &[u8]) -> String {
        String::from_utf8(octets.to_vec()).unwrap()
    }

    #[test]
    fn test_generer_plist() {
        let t = texte(&generer_plist(
            Path::new("/u/bin/discord-rich-claude"),
            Path::new("/l"),
        ));
        let attendu = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n<dict>\n\
\t<key>Label</key>\n\t<string>{LABEL}</string>\n\
\t<key>ProgramArguments</key>\n\t<array>\n\t\t<string>/u/bin/discord-rich-claude</string>\n\t\t<string>run</string>\n\t</array>\n\
\t<key>RunAtLoad</key>\n\t<true/>\n\
\t<key>StandardErrorPath</key>\n\t<string>/l/launchd.err.log</string>\n\
\t<key>StandardOutPath</key>\n\t<string>/l/launchd.out.log</string>\n\
</dict>\n</plist>\n"
        );
        assert_eq!(t, attendu);
        assert!(!t.contains("KeepAlive"));
        assert!(!t.contains("ProcessType"));
    }

    #[test]
    fn generer_plist_echappe_le_xml() {
        let t = texte(&generer_plist(Path::new("/a b/c&<>"), Path::new("/l")));
        assert!(t.contains("<string>/a b/c&amp;&lt;&gt;</string>"));
    }

    type Appels = Vec<Vec<String>>;

    fn faux_executeur<'a>(
        appels: &'a mut Appels,
        code_bootstrap: impl Fn(usize) -> i32 + 'a,
    ) -> impl FnMut(&[&str]) -> io::Result<Sortie> + 'a {
        move |cmd| {
            appels.push(cmd.iter().map(|s| s.to_string()).collect());
            let code = if cmd[1] == "bootstrap" {
                let n = appels.iter().filter(|c| c[1] == "bootstrap").count();
                code_bootstrap(n)
            } else {
                0
            };
            Ok(Sortie {
                code,
                stderr: b"boum".to_vec(),
            })
        }
    }

    #[test]
    fn test_installer_launch_agent() {
        let d = tempfile::tempdir().unwrap();
        let chemin = d.path().join("LaunchAgents").join(format!("{LABEL}.plist"));
        let mut appels = vec![];
        installer_launch_agent(
            &chemin,
            b"<plist/>",
            501,
            &mut faux_executeur(&mut appels, |_| 0),
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(fs::read(&chemin).unwrap(), b"<plist/>");
        let domaine = "gui/501".to_string();
        assert_eq!(
            appels,
            vec![
                vec![
                    LAUNCHCTL.to_string(),
                    "bootout".into(),
                    format!("{domaine}/{LABEL}")
                ],
                vec![
                    LAUNCHCTL.to_string(),
                    "bootstrap".into(),
                    domaine,
                    chemin.to_string_lossy().into_owned()
                ],
            ]
        );
    }

    #[test]
    fn test_desinstaller_launch_agent() {
        let d = tempfile::tempdir().unwrap();
        let chemin = d.path().join(format!("{LABEL}.plist"));
        fs::write(&chemin, b"<plist/>").unwrap();
        let mut appels = vec![];
        desinstaller_launch_agent(&chemin, 501, &mut faux_executeur(&mut appels, |_| 0)).unwrap();
        assert!(!chemin.exists());
        assert_eq!(
            appels,
            vec![vec![
                LAUNCHCTL.to_string(),
                "bootout".into(),
                format!("gui/501/{LABEL}")
            ]]
        );
        // déjà absent : pas d'erreur
        desinstaller_launch_agent(&chemin, 501, &mut faux_executeur(&mut vec![], |_| 0)).unwrap();
    }

    #[test]
    fn test_installer_launch_agent_ecrit_le_plist_avant_le_bootout() {
        let d = tempfile::tempdir().unwrap();
        let chemin = d.path().join("LaunchAgents").join(format!("{LABEL}.plist"));
        let mut vu = vec![];
        let mut executer = |_: &[&str]| -> io::Result<Sortie> {
            vu.push(fs::read(&chemin).unwrap());
            Ok(Sortie {
                code: 0,
                stderr: vec![],
            })
        };
        installer_launch_agent(&chemin, b"<plist/>", 501, &mut executer, &mut |_| {}).unwrap();
        assert!(vu.iter().all(|c| c == b"<plist/>") && vu.len() == 2);
    }

    #[test]
    fn test_installer_launch_agent_echec_ecriture_n_appelle_pas_launchctl() {
        // SAFETY : `geteuid` n'a aucun effet de bord. Root ignore les droits.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let dossier = d.path().join("LaunchAgents");
        fs::create_dir(&dossier).unwrap();
        let chemin = dossier.join(format!("{LABEL}.plist"));
        fs::set_permissions(&dossier, fs::Permissions::from_mode(0o500)).unwrap();
        let mut appels = vec![];
        let res = installer_launch_agent(
            &chemin,
            b"<plist/>",
            501,
            &mut faux_executeur(&mut appels, |_| 0),
            &mut |_| {},
        );
        fs::set_permissions(&dossier, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(matches!(res, Err(ErreurLaunchctl::Io(_))));
        assert!(appels.is_empty());
        assert!(!chemin.exists());
    }

    fn bootstraps(appels: &Appels) -> usize {
        appels.iter().filter(|c| c[1] == "bootstrap").count()
    }

    #[test]
    fn test_installer_launch_agent_reessaie_bootstrap_sur_code_5_puis_reussit() {
        let d = tempfile::tempdir().unwrap();
        let chemin = d.path().join("a").join("p.plist");
        let mut appels = vec![];
        let mut dodo = vec![];
        installer_launch_agent(
            &chemin,
            b"<plist/>",
            501,
            &mut faux_executeur(&mut appels, |n| if n <= 2 { 5 } else { 0 }),
            &mut |t| dodo.push(t),
        )
        .unwrap();
        assert_eq!(bootstraps(&appels), 3);
        assert_eq!(
            dodo,
            [Duration::from_millis(500), Duration::from_millis(500)]
        );
    }

    #[test]
    fn test_installer_launch_agent_reessaie_bootstrap_sur_code_37() {
        let d = tempfile::tempdir().unwrap();
        let chemin = d.path().join("a").join("p.plist");
        let mut appels = vec![];
        let mut dodo = vec![];
        installer_launch_agent(
            &chemin,
            b"<plist/>",
            501,
            &mut faux_executeur(&mut appels, |n| if n <= 1 { 37 } else { 0 }),
            &mut |t| dodo.push(t),
        )
        .unwrap();
        assert_eq!(bootstraps(&appels), 2);
        assert_eq!(dodo, [Duration::from_millis(500)]);
    }

    #[test]
    fn test_installer_launch_agent_echoue_apres_3_tentatives_sur_code_5() {
        let d = tempfile::tempdir().unwrap();
        let chemin = d.path().join("a").join("p.plist");
        let mut appels = vec![];
        let mut dodo = vec![];
        let res = installer_launch_agent(
            &chemin,
            b"<plist/>",
            501,
            &mut faux_executeur(&mut appels, |_| 5),
            &mut |t| dodo.push(t),
        );
        assert!(
            matches!(res, Err(ErreurLaunchctl::Echec { code: 5, ref stderr }) if stderr == "boum")
        );
        assert_eq!(bootstraps(&appels), 3);
        assert_eq!(dodo.len(), 2);
    }

    #[test]
    fn test_installer_launch_agent_autre_code_echoue_sans_reessai() {
        let d = tempfile::tempdir().unwrap();
        let chemin = d.path().join("a").join("p.plist");
        let mut appels = vec![];
        let mut dodo = vec![];
        let res = installer_launch_agent(
            &chemin,
            b"<plist/>",
            501,
            &mut faux_executeur(&mut appels, |_| 1),
            &mut |t| dodo.push(t),
        );
        assert!(matches!(res, Err(ErreurLaunchctl::Echec { code: 1, .. })));
        assert_eq!(bootstraps(&appels), 1);
        assert!(dodo.is_empty());
    }

    #[test]
    fn resultat_s_affiche_en_francais() {
        assert_eq!(Resultat::Inchange.to_string(), "inchangé");
        assert_eq!(Resultat::Refuse.to_string(), "refusé");
        assert_eq!(Resultat::Modifie.to_string(), "modifié");
    }
}

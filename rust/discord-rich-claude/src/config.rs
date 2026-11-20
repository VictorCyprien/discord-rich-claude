//! Lecture de config.toml. Voir la spec, section 7.1.
//!
//! Tout défaut de config retombe sur `Config::default()` : aucun projet
//! autorisé, pas de `client_id`, donc rien de montré (refus par défaut).

use std::ffi::{CStr, CString, OsStr, OsString};
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use crate::chemins::resoudre;

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub client_id: Option<String>,
    pub inactif_apres_min: f64,
    pub masquer_apres_min: f64,
    pub rotation_s: f64,
    pub projets_autorises: Vec<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            client_id: None,
            inactif_apres_min: 10.0,
            masquer_apres_min: 60.0,
            rotation_s: 20.0,
            projets_autorises: Vec::new(),
        }
    }
}

pub fn charger(chemin: &Path) -> Config {
    let octets = match std::fs::read(chemin) {
        Ok(o) => o,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            log::warn!("config absente : {}", chemin.display());
            return Config::default();
        }
        Err(e) => {
            log::warn!("config illisible, rien ne sera affiché : {:?}", e.kind());
            return Config::default();
        }
    };
    let texte = match String::from_utf8(octets) {
        Ok(t) => t,
        Err(_) => {
            log::warn!("config illisible, rien ne sera affiché : encodage invalide");
            return Config::default();
        }
    };
    let brut = match texte.parse::<toml::Table>() {
        Ok(t) => t,
        Err(e) => {
            log::warn!("config illisible, rien ne sera affiché : {}", e.message());
            return Config::default();
        }
    };
    let home = std::env::var_os("HOME");
    match valider(&brut, home.as_deref()) {
        Ok(c) => c,
        Err(message) => {
            log::warn!("config invalide, rien ne sera affiché : {message}");
            Config::default()
        }
    }
}

fn valider(brut: &toml::Table, home: Option<&OsStr>) -> Result<Config, String> {
    let client_id = match brut.get("client_id") {
        None => None,
        Some(toml::Value::String(s)) if s.is_empty() => None,
        Some(toml::Value::String(s)) => Some(s.clone()),
        Some(toml::Value::Integer(n)) => Some(n.to_string()),
        Some(_) => return Err("client_id doit être un texte".to_string()),
    };
    let autorises: Vec<&str> = match brut.get("projets") {
        None => Vec::new(),
        Some(toml::Value::Table(projets)) => match projets.get("autorises") {
            None => Vec::new(),
            Some(toml::Value::Array(liste)) => liste
                .iter()
                .map(toml::Value::as_str)
                .collect::<Option<Vec<&str>>>()
                .ok_or("projets.autorises doit être une liste de chemins")?,
            Some(_) => return Err("projets.autorises doit être une liste de chemins".to_string()),
        },
        Some(_) => return Err("[projets] doit être une table".to_string()),
    };
    let mut chemins = Vec::new();
    for p in autorises {
        let developpe =
            developper_tilde(p, home).ok_or_else(|| format!("chemin invalide refusé : {p}"))?;
        if !developpe.is_absolute() {
            return Err(format!("chemin relatif refusé : {p}"));
        }
        let resolu =
            resoudre(&developpe, false).map_err(|_| format!("chemin invalide refusé : {p}"))?;
        chemins.push(resolu);
    }
    Ok(Config {
        client_id,
        inactif_apres_min: borne(
            brut.get("inactif_apres_min"),
            10.0,
            "inactif_apres_min",
            1.0,
            1440.0,
        )?,
        masquer_apres_min: borne(
            brut.get("masquer_apres_min"),
            60.0,
            "masquer_apres_min",
            1.0,
            1440.0,
        )?,
        rotation_s: borne(brut.get("rotation_s"), 20.0, "rotation_s", 5.0, 3600.0)?,
        projets_autorises: chemins,
    })
}

/// Équivalent de `Path(p).expanduser()` : `~` et `~/…` passent par `HOME` (ou,
/// à défaut, par la base des utilisateurs), `~nom` par `getpwnam`. `None` si le
/// dossier personnel est introuvable (le `RuntimeError` de Python).
fn developper_tilde(p: &str, home: Option<&OsStr>) -> Option<PathBuf> {
    if !p.starts_with('~') {
        return Some(PathBuf::from(p));
    }
    let (premier, reste) = match p.find('/') {
        Some(i) => (&p[..i], &p[i..]),
        None => (p, ""),
    };
    let nom = &premier[1..];
    let dossier: OsString = if nom.is_empty() {
        match home {
            Some(h) => h.to_os_string(),
            None => dossier_utilisateur(None)?,
        }
    } else {
        dossier_utilisateur(Some(nom))?
    };
    // Comme `posixpath.expanduser` : les `/` finaux du dossier sont retirés.
    let mut octets = dossier.into_vec();
    while octets.last() == Some(&b'/') {
        octets.pop();
    }
    octets.extend_from_slice(reste.as_bytes());
    if octets.is_empty() {
        octets.push(b'/');
    }
    Some(PathBuf::from(OsString::from_vec(octets)))
}

/// Dossier personnel d'un utilisateur (`None` : l'utilisateur courant).
fn dossier_utilisateur(nom: Option<&str>) -> Option<OsString> {
    let mut tampon = vec![0u8; 4096];
    let mut pw: libc::passwd = unsafe { std::mem::zeroed() };
    let mut resultat: *mut libc::passwd = std::ptr::null_mut();
    let code = match nom {
        Some(n) => {
            let c = CString::new(n).ok()?;
            // SAFETY : pointeurs valides pour la durée de l'appel ; version réentrante.
            unsafe {
                libc::getpwnam_r(
                    c.as_ptr(),
                    &mut pw,
                    tampon.as_mut_ptr().cast(),
                    tampon.len(),
                    &mut resultat,
                )
            }
        }
        None => unsafe {
            libc::getpwuid_r(
                libc::getuid(),
                &mut pw,
                tampon.as_mut_ptr().cast(),
                tampon.len(),
                &mut resultat,
            )
        },
    };
    if code != 0 || resultat.is_null() || pw.pw_dir.is_null() {
        return None;
    }
    // SAFETY : `pw_dir` pointe dans `tampon`, texte terminé par NUL.
    let dir = unsafe { CStr::from_ptr(pw.pw_dir) };
    Some(OsStr::from_bytes(dir.to_bytes()).to_os_string())
}

/// Nombre (entier ou flottant, jamais un booléen) dans [mini, maxi]. Les bornes
/// sont testées avant la finitude, comme en Python (`nan` échoue la comparaison).
fn borne(
    valeur: Option<&toml::Value>,
    defaut: f64,
    nom: &str,
    mini: f64,
    maxi: f64,
) -> Result<f64, String> {
    let v = match valeur {
        None => defaut,
        Some(toml::Value::Integer(n)) => *n as f64,
        Some(toml::Value::Float(f)) => *f,
        Some(_) => return Err(format!("{nom} doit être un nombre")),
    };
    if !(mini <= v && v <= maxi) {
        return Err(format!("{nom} doit être compris entre {mini} et {maxi}"));
    }
    if !v.is_finite() {
        return Err(format!("{nom} doit être fini"));
    }
    Ok(v)
}

/// Date de modification en nanosecondes ; `None` si le fichier est inaccessible.
fn mtime(chemin: &Path) -> Option<i128> {
    let m = std::fs::metadata(chemin).ok()?;
    Some(i128::from(m.mtime()) * 1_000_000_000 + i128::from(m.mtime_nsec()))
}

/// Recharge la config quand la date de modification du fichier change.
pub struct ConfigSurveillee {
    pub chemin: PathBuf,
    pub valeur: Config,
    mtime: Option<i128>,
}

impl ConfigSurveillee {
    pub fn new(chemin: PathBuf) -> Self {
        let mtime = mtime(&chemin);
        let valeur = charger(&chemin);
        ConfigSurveillee {
            chemin,
            valeur,
            mtime,
        }
    }

    pub fn recharger_si_change(&mut self) -> bool {
        let nouveau = mtime(&self.chemin);
        if nouveau == self.mtime {
            return false;
        }
        self.mtime = nouveau;
        self.valeur = charger(&self.chemin);
        true
    }
}

pub const CONFIG_PAR_DEFAUT: &str = concat!(
    "# Configuration de discord-rich-claude.\n",
    "# ID de l'application Discord : Developer Portal > General Information > Application ID.\n",
    "client_id = \"\"\n",
    "inactif_apres_min = 10\n",
    "masquer_apres_min = 60\n",
    "rotation_s = 20\n",
    "\n",
    "[projets]\n",
    "# Refusé par défaut : seuls ces dossiers (chemins complets) affichent leur nom.\n",
    "autorises = []\n",
);

/// Création exclusive, 0600 : la config contiendra des chemins de projets.
///
/// `create_new` (`O_EXCL` avec `O_CREAT`) échoue si `chemin` existe déjà, même
/// comme lien symbolique pendant (vers une cible absente) : jamais suivi ni
/// écrasé.
pub fn creer_config_par_defaut(chemin: &Path) -> io::Result<bool> {
    if let Some(parent) = chemin.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(chemin)
    {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Ok(false),
        Err(e) => return Err(e),
    };
    f.write_all(CONFIG_PAR_DEFAUT.as_bytes())?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::time::Duration;

    fn ecrire(chemin: &Path, texte: &str) -> PathBuf {
        std::fs::write(chemin, texte).unwrap();
        chemin.to_path_buf()
    }

    fn dossier() -> (tempfile::TempDir, PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let r = t.path().canonicalize().unwrap();
        (t, r)
    }

    #[test]
    fn test_config_absente_refuse_tout() {
        let (_t, d) = dossier();
        assert_eq!(charger(&d.join("absente.toml")), Config::default());
    }

    #[test]
    fn test_config_complete() {
        let (_t, d) = dossier();
        let projet = d.join("projet");
        std::fs::create_dir(&projet).unwrap();
        let f = ecrire(
            &d.join("config.toml"),
            &format!(
                "client_id = \"123\"\ninactif_apres_min = 5\nrotation_s = 30\n\
                 [projets]\nautorises = [\"{}\"]\n",
                projet.display()
            ),
        );
        assert_eq!(
            charger(&f),
            Config {
                client_id: Some("123".into()),
                inactif_apres_min: 5.0,
                masquer_apres_min: 60.0,
                rotation_s: 30.0,
                projets_autorises: vec![projet.canonicalize().unwrap()],
            }
        );
    }

    #[test]
    fn test_client_id_entier_accepte() {
        let (_t, d) = dossier();
        let f = ecrire(&d.join("c.toml"), "client_id = 1234567890123456789\n");
        assert_eq!(
            charger(&f).client_id.as_deref(),
            Some("1234567890123456789")
        );
    }

    #[test]
    fn test_client_id_vide_vaut_absent() {
        let (_t, d) = dossier();
        let f = ecrire(&d.join("c.toml"), "client_id = \"\"\n");
        assert_eq!(charger(&f).client_id, None);
    }

    #[test]
    fn test_client_id_booleen_refuse_tout() {
        let (_t, d) = dossier();
        let f = ecrire(&d.join("c.toml"), "client_id = true\nrotation_s = 30\n");
        assert_eq!(charger(&f), Config::default());
    }

    #[test]
    fn test_toml_invalide_refuse_tout() {
        let (_t, d) = dossier();
        let f = ecrire(&d.join("c.toml"), "client_id = \n");
        assert_eq!(charger(&f), Config::default());
    }

    #[test]
    fn test_type_invalide_refuse_tout() {
        let (_t, d) = dossier();
        let f = ecrire(
            &d.join("c.toml"),
            "client_id = \"1\"\nrotation_s = \"vite\"\n",
        );
        assert_eq!(charger(&f), Config::default());
    }

    #[test]
    fn test_booleen_refuse_comme_nombre() {
        let (_t, d) = dossier();
        let f = ecrire(&d.join("c.toml"), "inactif_apres_min = true\n");
        assert_eq!(charger(&f), Config::default());
    }

    #[test]
    fn test_valeur_non_positive_refuse_tout() {
        let (_t, d) = dossier();
        let f = ecrire(&d.join("c.toml"), "inactif_apres_min = 0\n");
        assert_eq!(charger(&f), Config::default());
    }

    #[test]
    fn test_chemin_relatif_refuse_tout() {
        let (_t, d) = dossier();
        let f = ecrire(
            &d.join("c.toml"),
            "[projets]\nautorises = [\"relatif/projet\"]\n",
        );
        assert_eq!(charger(&f), Config::default());
    }

    #[test]
    fn test_encodage_invalide_refuse_tout() {
        let (_t, d) = dossier();
        let f = d.join("c.toml");
        std::fs::write(&f, b"client_id = \"\xff\xfe bad\"\n").unwrap();
        assert_eq!(charger(&f), Config::default());
    }

    #[test]
    fn test_chemin_utilisateur_inconnu_refuse_tout() {
        let (_t, d) = dossier();
        let f = ecrire(
            &d.join("c.toml"),
            "[projets]\nautorises = [\"~nonexistentuser999/projet\"]\n",
        );
        assert_eq!(charger(&f), Config::default());
    }

    #[test]
    fn test_tilde_passe_par_home() {
        let home = OsStr::new("/h/moi");
        assert_eq!(
            developper_tilde("~", Some(home)),
            Some(PathBuf::from("/h/moi"))
        );
        assert_eq!(
            developper_tilde("~/p", Some(home)),
            Some(PathBuf::from("/h/moi/p"))
        );
        assert_eq!(developper_tilde("~nonexistentuser999", Some(home)), None);
        assert_eq!(
            developper_tilde("/a/~b", Some(home)),
            Some(PathBuf::from("/a/~b"))
        );
    }

    #[test]
    fn test_valeur_non_finie_refuse_tout() {
        let (_t, d) = dossier();
        let f = ecrire(&d.join("c.toml"), "inactif_apres_min = nan\n");
        assert_eq!(charger(&f), Config::default());
        let f = ecrire(&d.join("c2.toml"), "rotation_s = inf\n");
        assert_eq!(charger(&f), Config::default());
    }

    #[test]
    fn test_entier_enorme_refuse_tout_sans_lever() {
        let (_t, d) = dossier();
        let f = ecrire(
            &d.join("c.toml"),
            &format!("inactif_apres_min = 1{}\n", "0".repeat(309)),
        );
        assert_eq!(charger(&f), Config::default());
    }

    #[test]
    fn test_rotation_s_enorme_refuse_tout_sans_lever() {
        let (_t, d) = dossier();
        let f = ecrire(
            &d.join("c.toml"),
            &format!("rotation_s = 1{}\n", "0".repeat(309)),
        );
        assert_eq!(charger(&f), Config::default());
    }

    #[test]
    fn test_rotation_s_minuscule_hors_bornes_refuse_tout() {
        let (_t, d) = dossier();
        let f = ecrire(&d.join("c.toml"), "rotation_s = 1e-320\n");
        assert_eq!(charger(&f), Config::default());
    }

    #[test]
    fn test_rotation_s_hors_bornes_5_3600() {
        let (_t, d) = dossier();
        for valeur in [4, 3601] {
            let f = ecrire(&d.join("c.toml"), &format!("rotation_s = {valeur}\n"));
            assert_eq!(charger(&f), Config::default());
        }
    }

    #[test]
    fn test_rotation_s_aux_bornes_acceptee() {
        let (_t, d) = dossier();
        for valeur in [5, 3600] {
            let f = ecrire(&d.join("c.toml"), &format!("rotation_s = {valeur}\n"));
            assert_eq!(charger(&f).rotation_s, f64::from(valeur));
        }
    }

    #[test]
    fn test_inactif_apres_min_hors_bornes_1_1440() {
        let (_t, d) = dossier();
        for valeur in [0, 1441] {
            let f = ecrire(
                &d.join("c.toml"),
                &format!("inactif_apres_min = {valeur}\n"),
            );
            assert_eq!(charger(&f), Config::default());
        }
    }

    #[test]
    fn test_inactif_apres_min_aux_bornes_acceptee() {
        let (_t, d) = dossier();
        for valeur in [1, 1440] {
            let f = ecrire(
                &d.join("c.toml"),
                &format!("inactif_apres_min = {valeur}\n"),
            );
            assert_eq!(charger(&f).inactif_apres_min, f64::from(valeur));
        }
    }

    #[test]
    fn test_client_id_absent_garde_la_liste_blanche() {
        let (_t, d) = dossier();
        let projet = d.join("p");
        std::fs::create_dir(&projet).unwrap();
        let f = ecrire(
            &d.join("c.toml"),
            &format!("[projets]\nautorises = [\"{}\"]\n", projet.display()),
        );
        let c = charger(&f);
        assert_eq!(c.client_id, None);
        assert_eq!(c.projets_autorises, vec![projet.canonicalize().unwrap()]);
    }

    #[test]
    fn test_rechargement_seulement_si_le_fichier_change() {
        let (_t, d) = dossier();
        let f = ecrire(&d.join("c.toml"), "client_id = \"1\"\n");
        let mut cs = ConfigSurveillee::new(f.clone());
        assert_eq!(cs.valeur.client_id.as_deref(), Some("1"));
        assert!(!cs.recharger_si_change());
        ecrire(&f, "client_id = \"2\"\n");
        let m = std::fs::metadata(&f).unwrap().modified().unwrap();
        let fichier = OpenOptions::new().write(true).open(&f).unwrap();
        fichier.set_modified(m + Duration::from_secs(1)).unwrap();
        assert!(cs.recharger_si_change());
        assert_eq!(cs.valeur.client_id.as_deref(), Some("2"));
    }

    #[test]
    fn test_rechargement_quand_le_fichier_disparait() {
        let (_t, d) = dossier();
        let f = ecrire(&d.join("c.toml"), "client_id = \"1\"\n");
        let mut cs = ConfigSurveillee::new(f.clone());
        std::fs::remove_file(&f).unwrap();
        assert!(cs.recharger_si_change());
        assert_eq!(cs.valeur, Config::default());
    }

    #[test]
    fn test_config_par_defaut_creee_et_refuse_tout() {
        let (_t, d) = dossier();
        let f = d.join("sous").join("config.toml");
        assert!(creer_config_par_defaut(&f).unwrap());
        assert_eq!(charger(&f), Config::default());
    }

    #[test]
    fn test_config_par_defaut_n_ecrase_pas() {
        let (_t, d) = dossier();
        let f = ecrire(&d.join("config.toml"), "client_id = \"1\"\n");
        assert!(!creer_config_par_defaut(&f).unwrap());
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "client_id = \"1\"\n");
    }

    #[test]
    fn test_config_par_defaut_creee_en_0600() {
        let (_t, d) = dossier();
        let f = d.join("config.toml");
        assert!(creer_config_par_defaut(&f).unwrap());
        let mode = std::fs::metadata(&f).unwrap().permissions().mode();
        assert_eq!(mode & 0o7777, 0o600);
    }

    #[test]
    fn test_config_par_defaut_lien_symbolique_pendant_refuse_sans_toucher_la_cible() {
        let (_t, d) = dossier();
        let cible = d.join("cible-absente.toml");
        let lien = d.join("config.toml");
        symlink(&cible, &lien).unwrap();
        assert!(!creer_config_par_defaut(&lien).unwrap());
        assert!(!cible.exists());
    }

    #[test]
    fn test_masquer_apres_min_defaut() {
        let (_t, d) = dossier();
        assert_eq!(charger(&d.join("absente.toml")).masquer_apres_min, 60.0);
    }

    #[test]
    fn test_masquer_apres_min_valeur_valide_lue() {
        let (_t, d) = dossier();
        let f = ecrire(&d.join("c.toml"), "masquer_apres_min = 30\n");
        assert_eq!(charger(&f).masquer_apres_min, 30.0);
    }

    #[test]
    fn test_masquer_apres_min_hors_bornes_1_1440() {
        let (_t, d) = dossier();
        for valeur in [0, 1441] {
            let f = ecrire(
                &d.join("c.toml"),
                &format!("masquer_apres_min = {valeur}\n"),
            );
            assert_eq!(charger(&f), Config::default());
        }
    }

    #[test]
    fn test_masquer_apres_min_aux_bornes_acceptee() {
        let (_t, d) = dossier();
        for valeur in [1, 1440] {
            let f = ecrire(
                &d.join("c.toml"),
                &format!("masquer_apres_min = {valeur}\n"),
            );
            assert_eq!(charger(&f).masquer_apres_min, f64::from(valeur));
        }
    }

    #[test]
    fn test_masquer_apres_min_chaine_refuse_tout() {
        let (_t, d) = dossier();
        let f = ecrire(&d.join("c.toml"), "masquer_apres_min = \"60\"\n");
        assert_eq!(charger(&f), Config::default());
    }

    #[test]
    fn test_masquer_apres_min_booleen_refuse_tout() {
        let (_t, d) = dossier();
        let f = ecrire(&d.join("c.toml"), "masquer_apres_min = true\n");
        assert_eq!(charger(&f), Config::default());
    }

    #[test]
    fn test_masquer_apres_min_non_fini_refuse_tout() {
        let (_t, d) = dossier();
        let f = ecrire(&d.join("c.toml"), "masquer_apres_min = nan\n");
        assert_eq!(charger(&f), Config::default());
    }

    #[test]
    fn test_masquer_apres_min_enorme_refuse_tout_sans_lever() {
        let (_t, d) = dossier();
        let f = ecrire(
            &d.join("c.toml"),
            &format!("masquer_apres_min = 1{}\n", "0".repeat(400)),
        );
        assert_eq!(charger(&f), Config::default());
    }

    #[test]
    fn test_config_par_defaut_texte_exact() {
        assert!(CONFIG_PAR_DEFAUT.starts_with("# Configuration de discord-rich-claude.\n"));
        assert!(CONFIG_PAR_DEFAUT.ends_with("autorises = []\n"));
    }
}

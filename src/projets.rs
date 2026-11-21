//! Configuration éditable : lecture de `config.toml`, opérations sur la liste
//! blanche des projets, réécriture complète du fichier. Voir la spec, section 15.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use crate::chemins::resoudre;
use crate::config::{Config, alias_valide, nom_affiche, resoudre_entree, valider};
use crate::installation::ecrire_fichier_atomique;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErreurEdition {
    DejaPresent,
    NomInvalide,
    ConfigInvalide,
    LienSymbolique,
    Ecriture,
    ProjetAbsent,
    DossierTropLarge,
}

impl ErreurEdition {
    /// Texte de l'alerte (spec 15.2) ; `ProjetAbsent` ne s'affiche pas.
    pub fn message(self) -> Option<&'static str> {
        match self {
            ErreurEdition::DejaPresent => Some("Ce dossier est déjà dans la liste."),
            ErreurEdition::NomInvalide => {
                Some("Nom invalide : 1 à 32 caractères, sans caractère de contrôle.")
            }
            ErreurEdition::ConfigInvalide => {
                Some("config.toml est invalide : corrige-la avant de modifier les projets.")
            }
            ErreurEdition::LienSymbolique => {
                Some("config.toml est un lien symbolique : modifie-la à la main.")
            }
            ErreurEdition::Ecriture => Some("Impossible d'écrire config.toml (voir le journal)."),
            ErreurEdition::DossierTropLarge => {
                Some("Choisis un dossier de projet, pas la racine ni le dossier personnel.")
            }
            ErreurEdition::ProjetAbsent => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjetAffiche {
    pub nom: String,
    /// Chemin tel qu'écrit dans `config.toml` : la clé d'identité.
    pub chemin: String,
    /// Chemin à montrer : `~/…` sous le dossier personnel.
    pub chemin_affiche: String,
}

#[derive(Clone, Debug)]
struct Entree {
    /// Chaîne telle qu'écrite dans `autorises`.
    forme: String,
    resolu: PathBuf,
    alias: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ConfigEditable {
    client_id: Option<toml::Value>,
    inactif_apres_min: Option<toml::Value>,
    masquer_apres_min: Option<toml::Value>,
    rotation_s: Option<toml::Value>,
    entrees: Vec<Entree>,
}

/// Dernier élément du chemin résolu.
pub fn nom_dossier(dossier: &Path) -> String {
    let resolu = resoudre(dossier, false).unwrap_or_else(|_| dossier.to_path_buf());
    resolu
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Forme écrite dans `config.toml` : `~/reste` sous `home`, sinon le chemin
/// absolu, sans `/` final (sauf pour `/`).
pub fn forme_ecrite(dossier: &Path, home: Option<&OsStr>) -> String {
    let brut = dossier.to_string_lossy();
    let sans_final = match brut.trim_end_matches('/') {
        "" if brut.starts_with('/') => "/",
        s => s,
    };
    if let Some(h) = home {
        let h = h.to_string_lossy();
        let h = h.trim_end_matches('/');
        if !h.is_empty() {
            if sans_final == h {
                return "~".to_string();
            }
            if let Some(reste) = sans_final.strip_prefix(h)
                && reste.starts_with('/')
            {
                return format!("~{reste}");
            }
        }
    }
    sans_final.to_string()
}

/// Chaîne TOML de base `"…"`, toujours la même forme : `\\`, `\"` et tout
/// caractère de contrôle en `\uXXXX`.
fn cle_toml(s: &str) -> String {
    let mut sortie = String::with_capacity(s.len() + 2);
    sortie.push('"');
    for c in s.chars() {
        match c {
            '\\' => sortie.push_str("\\\\"),
            '"' => sortie.push_str("\\\""),
            c if c.is_control() => sortie.push_str(&format!("\\u{:04X}", c as u32)),
            c => sortie.push(c),
        }
    }
    sortie.push('"');
    sortie
}

/// `/` et le dossier personnel (chemins résolus) sont trop larges pour un projet.
fn trop_large(resolu: &Path, home: Option<&OsStr>) -> bool {
    resolu == Path::new("/")
        || home
            .and_then(|h| resoudre(Path::new(h), false).ok())
            .is_some_and(|h| h == resolu)
}

impl ConfigEditable {
    pub fn lire(chemin: &Path, home: Option<&OsStr>) -> Result<Self, ErreurEdition> {
        match fs::symlink_metadata(chemin) {
            Ok(m) if m.file_type().is_symlink() => return Err(ErreurEdition::LienSymbolique),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(ConfigEditable {
                    client_id: None,
                    inactif_apres_min: None,
                    masquer_apres_min: None,
                    rotation_s: None,
                    entrees: Vec::new(),
                });
            }
            Err(_) => return Err(ErreurEdition::ConfigInvalide),
        }
        let texte = fs::read_to_string(chemin).map_err(|_| ErreurEdition::ConfigInvalide)?;
        let brut = texte
            .parse::<toml::Table>()
            .map_err(|_| ErreurEdition::ConfigInvalide)?;
        let config = valider(&brut, home).map_err(|_| ErreurEdition::ConfigInvalide)?;
        let formes: Vec<&str> = brut
            .get("projets")
            .and_then(|p| p.get("autorises"))
            .and_then(toml::Value::as_array)
            .map(|liste| liste.iter().filter_map(toml::Value::as_str).collect())
            .unwrap_or_default();
        let entrees = formes
            .into_iter()
            .zip(config.projets_autorises)
            .map(|(forme, resolu)| Entree {
                forme: forme.to_string(),
                alias: config.alias.get(&resolu).cloned(),
                resolu,
            })
            .collect();
        Ok(ConfigEditable {
            client_id: brut.get("client_id").cloned(),
            inactif_apres_min: brut.get("inactif_apres_min").cloned(),
            masquer_apres_min: brut.get("masquer_apres_min").cloned(),
            rotation_s: brut.get("rotation_s").cloned(),
            entrees,
        })
    }

    /// Alias par chemin résolu, pour l'aide commune de nom affiché.
    fn alias_par_chemin(&self) -> BTreeMap<PathBuf, String> {
        self.entrees
            .iter()
            .filter_map(|e| Some((e.resolu.clone(), e.alias.clone()?)))
            .collect()
    }

    pub fn affiches(&self, home: Option<&OsStr>) -> Vec<ProjetAffiche> {
        let alias = self.alias_par_chemin();
        let mut liste: Vec<ProjetAffiche> = self
            .entrees
            .iter()
            .map(|e| ProjetAffiche {
                nom: nom_affiche(&e.resolu, &alias),
                chemin: e.forme.clone(),
                chemin_affiche: forme_ecrite(Path::new(&e.forme), home),
            })
            .collect();
        liste.sort_by_key(|p| p.nom.to_lowercase());
        liste
    }

    pub fn nom_affiche(&self, chemin: &str) -> Option<String> {
        self.entrees
            .iter()
            .find(|e| e.forme == chemin)
            .map(|e| nom_affiche(&e.resolu, &self.alias_par_chemin()))
    }

    pub fn contient(&self, dossier: &Path, home: Option<&OsStr>) -> bool {
        resoudre_entree(&forme_ecrite(dossier, home), home)
            .is_ok_and(|r| self.entrees.iter().any(|e| e.resolu == r))
    }

    pub fn ajouter(
        &mut self,
        dossier: &Path,
        saisi: &str,
        home: Option<&OsStr>,
    ) -> Result<(), ErreurEdition> {
        let forme = forme_ecrite(dossier, home);
        let resolu = resoudre_entree(&forme, home).map_err(|_| ErreurEdition::ProjetAbsent)?;
        if trop_large(&resolu, home) {
            return Err(ErreurEdition::DossierTropLarge);
        }
        if self.entrees.iter().any(|e| e.resolu == resolu) {
            return Err(ErreurEdition::DejaPresent);
        }
        let alias = alias_saisi(saisi, &resolu)?;
        self.entrees.push(Entree {
            forme,
            resolu,
            alias,
        });
        Ok(())
    }

    pub fn renommer(&mut self, chemin: &str, saisi: &str) -> Result<(), ErreurEdition> {
        let i = self.position(chemin)?;
        let alias = alias_saisi(saisi, &self.entrees[i].resolu)?;
        self.entrees[i].alias = alias;
        Ok(())
    }

    pub fn changer_dossier(
        &mut self,
        chemin: &str,
        nouveau: &Path,
        home: Option<&OsStr>,
    ) -> Result<(), ErreurEdition> {
        let i = self.position(chemin)?;
        let forme = forme_ecrite(nouveau, home);
        let resolu = resoudre_entree(&forme, home).map_err(|_| ErreurEdition::ProjetAbsent)?;
        if trop_large(&resolu, home) {
            return Err(ErreurEdition::DossierTropLarge);
        }
        if self.entrees.iter().any(|e| e.resolu == resolu) {
            return Err(ErreurEdition::DejaPresent);
        }
        self.entrees[i].forme = forme;
        self.entrees[i].resolu = resolu;
        Ok(())
    }

    pub fn retirer(&mut self, chemin: &str) -> Result<(), ErreurEdition> {
        let i = self.position(chemin)?;
        self.entrees.remove(i);
        Ok(())
    }

    fn position(&self, chemin: &str) -> Result<usize, ErreurEdition> {
        self.entrees
            .iter()
            .position(|e| e.forme == chemin)
            .ok_or(ErreurEdition::ProjetAbsent)
    }

    /// Texte complet de `config.toml`.
    pub fn rendre(&self) -> String {
        let texte = |v: &Option<toml::Value>, defaut: &str| {
            v.as_ref()
                .map_or_else(|| defaut.to_string(), |v| v.to_string())
        };
        let mut s = String::new();
        s.push_str("# Configuration de discord-rich-claude.\n");
        s.push_str(
            "# ID de l'application Discord : Developer Portal > General Information > Application ID.\n",
        );
        s.push_str(&format!("client_id = {}\n", texte(&self.client_id, "\"\"")));
        s.push_str(&format!(
            "inactif_apres_min = {}\n",
            texte(&self.inactif_apres_min, "10")
        ));
        s.push_str(&format!(
            "masquer_apres_min = {}\n",
            texte(&self.masquer_apres_min, "60")
        ));
        s.push_str(&format!("rotation_s = {}\n", texte(&self.rotation_s, "20")));
        s.push_str("\n[projets]\n");
        s.push_str(
            "# Refusé par défaut : seuls ces dossiers (chemins complets) affichent leur nom.\n",
        );
        if self.entrees.is_empty() {
            s.push_str("autorises = []\n");
        } else {
            s.push_str("autorises = [\n");
            for e in &self.entrees {
                s.push_str(&format!("  {},\n", cle_toml(&e.forme)));
            }
            s.push_str("]\n");
        }
        if self.entrees.iter().any(|e| e.alias.is_some()) {
            s.push_str("\n[projets.alias]\n");
            s.push_str("# Nom affiché sur Discord à la place du nom du dossier.\n");
            // Une clé répétée rendrait le fichier illisible : une seule ligne par forme.
            let mut ecrites: Vec<&str> = Vec::new();
            for e in &self.entrees {
                if let Some(alias) = &e.alias
                    && !ecrites.contains(&e.forme.as_str())
                {
                    ecrites.push(&e.forme);
                    s.push_str(&format!("{} = {}\n", cle_toml(&e.forme), cle_toml(alias)));
                }
            }
        }
        s
    }

    /// Réécrit `cible` en entier : atomique, 0600, jamais à travers un lien.
    ///
    /// Le texte rendu est relu et validé avant d'écrire : s'il ne redonne pas la
    /// même liste blanche et les mêmes alias, rien n'est écrit.
    pub fn ecrire(&self, cible: &Path, home: Option<&OsStr>) -> Result<(), ErreurEdition> {
        if fs::symlink_metadata(cible).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(ErreurEdition::LienSymbolique);
        }
        let texte = self.rendre();
        if !self.relecture_fidele(&texte, home) {
            log::warn!("config rendue invalide, rien écrit");
            return Err(ErreurEdition::Ecriture);
        }
        let resultat = match cible.parent() {
            Some(parent) => fs::create_dir_all(parent),
            None => Ok(()),
        }
        .and_then(|_| ecrire_fichier_atomique(cible, texte.as_bytes(), 0o600));
        resultat.map_err(|e| {
            log::warn!("écriture de config.toml impossible : {:?}", e.kind());
            ErreurEdition::Ecriture
        })
    }
}

impl ConfigEditable {
    fn relecture_fidele(&self, texte: &str, home: Option<&OsStr>) -> bool {
        let Ok(table) = texte.parse::<toml::Table>() else {
            return false;
        };
        let Ok(Config {
            projets_autorises,
            alias,
            ..
        }) = valider(&table, home)
        else {
            return false;
        };
        let attendus: Vec<&PathBuf> = self.entrees.iter().map(|e| &e.resolu).collect();
        projets_autorises.iter().eq(attendus) && alias == self.alias_par_chemin()
    }
}

/// Alias issu d'une saisie : vide ou égal au nom du dossier, pas d'alias.
fn alias_saisi(saisi: &str, resolu: &Path) -> Result<Option<String>, ErreurEdition> {
    let nom = saisi.trim();
    if nom.is_empty() || nom == nom_dossier(resolu) {
        Ok(None)
    } else if alias_valide(nom) {
        Ok(Some(nom.to_string()))
    } else {
        Err(ErreurEdition::NomInvalide)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use tempfile::TempDir;

    fn dossier(home: &Path, nom: &str) -> PathBuf {
        let d = home.join(nom);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn lire_texte(home: &Path, texte: &str) -> Result<ConfigEditable, ErreurEdition> {
        let f = home.join("config.toml");
        fs::write(&f, texte).unwrap();
        ConfigEditable::lire(&f, Some(home.as_os_str()))
    }

    fn vide() -> ConfigEditable {
        ConfigEditable::lire(Path::new("/inexistant/config.toml"), None).unwrap()
    }

    fn avec(entrees: &[(&str, Option<&str>)]) -> ConfigEditable {
        let mut c = vide();
        c.client_id = Some(toml::Value::String("123".into()));
        c.entrees = entrees
            .iter()
            .map(|(forme, alias)| Entree {
                forme: forme.to_string(),
                resolu: PathBuf::from(forme),
                alias: alias.map(str::to_string),
            })
            .collect();
        c
    }

    #[test]
    fn rendre_format_exact() {
        let c = avec(&[
            ("~/W/AURA", None),
            ("~/W/discord-rich-claude", Some("DiscordRichClaude")),
        ]);
        let attendu = "\
# Configuration de discord-rich-claude.
# ID de l'application Discord : Developer Portal > General Information > Application ID.
client_id = \"123\"
inactif_apres_min = 10
masquer_apres_min = 60
rotation_s = 20

[projets]
# Refusé par défaut : seuls ces dossiers (chemins complets) affichent leur nom.
autorises = [
  \"~/W/AURA\",
  \"~/W/discord-rich-claude\",
]

[projets.alias]
# Nom affiché sur Discord à la place du nom du dossier.
\"~/W/discord-rich-claude\" = \"DiscordRichClaude\"
";
        assert_eq!(c.rendre(), attendu);
    }

    #[test]
    fn config_absente_rendue_a_l_octet_pres() {
        assert_eq!(vide().rendre(), crate::config::CONFIG_PAR_DEFAUT);
    }

    #[test]
    fn rendre_sans_alias_omet_la_table() {
        let c = avec(&[("~/a", None)]);
        assert!(!c.rendre().contains("[projets.alias]"));
    }

    #[test]
    fn rendre_liste_vide() {
        let c = avec(&[]);
        assert!(c.rendre().contains("autorises = []\n"));
        assert!(!c.rendre().contains("[projets.alias]"));
    }

    #[test]
    fn lire_absent_valeurs_par_defaut() {
        let rendu = vide().rendre();
        for ligne in [
            "client_id = \"\"",
            "inactif_apres_min = 10",
            "masquer_apres_min = 60",
            "rotation_s = 20",
            "autorises = []",
        ] {
            assert!(rendu.contains(ligne), "{ligne} absent de :\n{rendu}");
        }
    }

    #[test]
    fn lire_garde_la_forme_ecrite() {
        let home = TempDir::new().unwrap();
        dossier(home.path(), "p");
        let c = lire_texte(
            home.path(),
            "client_id = 123\ninactif_apres_min = 10.5\n[projets]\nautorises = [\"~/p\"]\n",
        )
        .unwrap();
        let rendu = c.rendre();
        assert!(rendu.contains("client_id = 123\n"));
        assert!(rendu.contains("inactif_apres_min = 10.5\n"));
        assert!(rendu.contains("  \"~/p\",\n"));
        assert_eq!(c.affiches(None)[0].chemin, "~/p");
    }

    #[test]
    fn lire_config_invalide() {
        let home = TempDir::new().unwrap();
        for texte in [
            "ceci n'est pas du toml",
            "[projets]\nautorises = [\"relatif\"]\n",
        ] {
            assert_eq!(
                lire_texte(home.path(), texte).unwrap_err(),
                ErreurEdition::ConfigInvalide
            );
        }
    }

    #[test]
    fn lire_lien_symbolique() {
        let tmp = TempDir::new().unwrap();
        let vrai = tmp.path().join("vrai.toml");
        fs::write(&vrai, "").unwrap();
        let lien = tmp.path().join("config.toml");
        symlink(&vrai, &lien).unwrap();
        assert_eq!(
            ConfigEditable::lire(&lien, None).unwrap_err(),
            ErreurEdition::LienSymbolique
        );
    }

    #[test]
    fn forme_ecrite_tilde_sous_home() {
        let home = Path::new("/Users/x");
        let h = Some(home.as_os_str());
        assert_eq!(forme_ecrite(Path::new("/Users/x/W/X/"), h), "~/W/X");
        assert_eq!(forme_ecrite(Path::new("/Users/x"), h), "~");
        assert_eq!(forme_ecrite(Path::new("/opt/X"), h), "/opt/X");
        assert_eq!(forme_ecrite(Path::new("/Users/xy/Z"), h), "/Users/xy/Z");
        assert_eq!(forme_ecrite(Path::new("/"), h), "/");
    }

    #[test]
    fn ajouter_deja_present_compare_les_chemins_resolus() {
        let home = TempDir::new().unwrap();
        let reel = dossier(home.path(), "reel");
        let lien = home.path().join("lien");
        symlink(&reel, &lien).unwrap();
        let h = Some(home.path().as_os_str());
        let mut c = vide();
        c.ajouter(&reel, "", h).unwrap();
        assert!(c.contient(&lien, h));
        assert_eq!(c.ajouter(&lien, "", h), Err(ErreurEdition::DejaPresent));
        assert_eq!(c.affiches(None).len(), 1);
    }

    #[test]
    fn ajouter_nom_vide_ou_egal_au_dossier_sans_alias() {
        let home = TempDir::new().unwrap();
        let h = Some(home.path().as_os_str());
        let mut c = vide();
        c.ajouter(&dossier(home.path(), "a"), "  ", h).unwrap();
        c.ajouter(&dossier(home.path(), "b"), " b ", h).unwrap();
        assert!(!c.rendre().contains("[projets.alias]"));
    }

    #[test]
    fn renommer_pose_puis_retire_l_alias() {
        let mut c = avec(&[("/p/a", None)]);
        c.renommer("/p/a", " Beau ").unwrap();
        assert_eq!(c.nom_affiche("/p/a").as_deref(), Some("Beau"));
        assert!(c.rendre().contains("\"/p/a\" = \"Beau\""));
        c.renommer("/p/a", "a").unwrap();
        assert_eq!(c.nom_affiche("/p/a").as_deref(), Some("a"));
        assert!(!c.rendre().contains("[projets.alias]"));
    }

    #[test]
    fn renommer_33_caracteres_refuse_sans_changer() {
        let mut c = avec(&[("/p/a", Some("Avant"))]);
        let avant = c.rendre();
        assert_eq!(
            c.renommer("/p/a", &"x".repeat(33)),
            Err(ErreurEdition::NomInvalide)
        );
        assert_eq!(
            c.renommer("/p/a", "tab\tulation"),
            Err(ErreurEdition::NomInvalide)
        );
        assert_eq!(c.rendre(), avant);
    }

    #[test]
    fn changer_dossier_garde_l_alias() {
        let home = TempDir::new().unwrap();
        let h = Some(home.path().as_os_str());
        let mut c = vide();
        c.ajouter(&dossier(home.path(), "a"), "Zed", h).unwrap();
        c.changer_dossier("~/a", &dossier(home.path(), "b"), h)
            .unwrap();
        let rendu = c.rendre();
        assert!(rendu.contains("\"~/b\" = \"Zed\""));
        assert!(!rendu.contains("~/a"));
    }

    #[test]
    fn changer_dossier_vers_un_dossier_deja_present() {
        let home = TempDir::new().unwrap();
        let h = Some(home.path().as_os_str());
        let mut c = vide();
        c.ajouter(&dossier(home.path(), "a"), "", h).unwrap();
        c.ajouter(&dossier(home.path(), "b"), "", h).unwrap();
        let avant = c.rendre();
        assert_eq!(
            c.changer_dossier("~/a", &home.path().join("b"), h),
            Err(ErreurEdition::DejaPresent)
        );
        assert_eq!(c.rendre(), avant);
    }

    #[test]
    fn retirer_retire_aussi_l_alias() {
        let mut c = avec(&[("/p/a", Some("A2")), ("/p/b", None)]);
        c.retirer("/p/a").unwrap();
        let rendu = c.rendre();
        assert!(!rendu.contains("/p/a"));
        assert!(!rendu.contains("A2"));
        assert!(rendu.contains("/p/b"));
    }

    #[test]
    fn chemin_inconnu_projet_absent() {
        let home = TempDir::new().unwrap();
        let h = Some(home.path().as_os_str());
        let mut c = avec(&[("/p/a", None)]);
        assert_eq!(c.renommer("/x", "n"), Err(ErreurEdition::ProjetAbsent));
        assert_eq!(
            c.changer_dossier("/x", home.path(), h),
            Err(ErreurEdition::ProjetAbsent)
        );
        assert_eq!(c.retirer("/x"), Err(ErreurEdition::ProjetAbsent));
        assert_eq!(ErreurEdition::ProjetAbsent.message(), None);
    }

    #[test]
    fn affiches_tries_sans_casse_et_alias_prioritaire() {
        let home = TempDir::new().unwrap();
        let h = Some(home.path().as_os_str());
        let mut c = vide();
        c.ajouter(&dossier(home.path(), "b"), "", h).unwrap();
        c.ajouter(&dossier(home.path(), "A"), "", h).unwrap();
        c.ajouter(&dossier(home.path(), "c"), "Zed", h).unwrap();
        let noms: Vec<String> = c.affiches(None).into_iter().map(|p| p.nom).collect();
        assert_eq!(noms, ["A", "b", "Zed"]);
    }

    #[test]
    fn aller_retour_caracteres_speciaux() {
        let home = TempDir::new().unwrap();
        let h = Some(home.path().as_os_str());
        let dossier = dossier(home.path(), "Mon \"dossier\" l'été \\ 🦀 «x»");
        let alias = "Mon \"Projet\" l'a \\ é🦀 «x»";
        let mut c = vide();
        c.ajouter(&dossier, alias, h).unwrap();
        let cible = home.path().join("sortie.toml");
        c.ecrire(&cible, h).unwrap();

        let relu = ConfigEditable::lire(&cible, h).unwrap();
        assert_eq!(relu.affiches(None)[0].nom, alias);

        let texte = fs::read_to_string(&cible).unwrap();
        let config = valider(&texte.parse::<toml::Table>().unwrap(), h).unwrap();
        let resolu = resoudre(&dossier, false).unwrap();
        assert_eq!(
            config.projets_autorises.as_slice(),
            std::slice::from_ref(&resolu)
        );
        assert_eq!(config.alias.get(&resolu).map(String::as_str), Some(alias));

        let sous = dossier.join("src");
        fs::create_dir_all(&sous).unwrap();
        assert_eq!(
            crate::carte::nom_projet(sous.to_str(), &config.projets_autorises, &config.alias),
            alias
        );
    }

    #[test]
    fn doublon_dans_autorises_reste_lisible_apres_ajout() {
        let home = TempDir::new().unwrap();
        let h = Some(home.path().as_os_str());
        let p = dossier(home.path(), "p");
        let q = dossier(home.path(), "q");
        let abs = p.display();
        let mut c = lire_texte(
            home.path(),
            &format!(
                "[projets]\nautorises = [\"{abs}\", \"{abs}\"]\n[projets.alias]\n\"{abs}\" = \"Pp\"\n"
            ),
        )
        .unwrap();
        c.ajouter(&q, "", h).unwrap();
        let cible = home.path().join("sortie.toml");
        c.ecrire(&cible, h).unwrap();
        let texte = fs::read_to_string(&cible).unwrap();
        assert_eq!(texte.matches("Pp").count(), 1, "{texte}");
        let relu = ConfigEditable::lire(&cible, h).unwrap();
        assert_eq!(relu.affiches(None).len(), 3);
    }

    #[test]
    fn cle_avec_antislash_apostrophe_et_saut_de_ligne() {
        let home = TempDir::new().unwrap();
        let h = Some(home.path().as_os_str());
        let mut c = vide();
        c.ajouter(&dossier(home.path(), "a\\b'c"), "Zed", h)
            .unwrap();
        let table = c.rendre().parse::<toml::Table>().unwrap();
        let config = valider(&table, h).unwrap();
        assert_eq!(config.alias.values().collect::<Vec<_>>(), ["Zed"]);

        let mut c = avec(&[("/p/a\nb\tc\u{7f}", Some("N"))]);
        c.entrees[0].resolu = PathBuf::from("/p/a\nb\tc\u{7f}");
        let rendu = c.rendre();
        assert!(!rendu.contains("a\nb"), "{rendu}");
        let table = rendu.parse::<toml::Table>().unwrap();
        let config = valider(&table, None).unwrap();
        assert_eq!(config.alias.len(), 1);
    }

    #[test]
    fn cle_toml_echappe_tout() {
        assert_eq!(
            cle_toml("a\\b\"c'd\n\u{1}"),
            "\"a\\\\b\\\"c'd\\u000A\\u0001\""
        );
    }

    #[test]
    fn ecrire_refuse_une_config_rendue_invalide() {
        let tmp = TempDir::new().unwrap();
        let cible = tmp.path().join("config.toml");
        // Forme relative : la relecture la refuse.
        let c = avec(&[("relatif", None)]);
        assert_eq!(c.ecrire(&cible, None), Err(ErreurEdition::Ecriture));
        assert!(!cible.exists());
    }

    #[test]
    fn ecrire_refuse_si_la_relecture_differe() {
        let tmp = TempDir::new().unwrap();
        let cible = tmp.path().join("config.toml");
        let mut c = avec(&[("/p/a", None)]);
        c.entrees[0].resolu = PathBuf::from("/p/autre");
        assert_eq!(c.ecrire(&cible, None), Err(ErreurEdition::Ecriture));
        assert!(!cible.exists());
    }

    #[test]
    fn racine_et_dossier_personnel_refuses() {
        let home = TempDir::new().unwrap();
        let h = Some(home.path().as_os_str());
        let mut c = vide();
        assert_eq!(
            c.ajouter(Path::new("/"), "", h),
            Err(ErreurEdition::DossierTropLarge)
        );
        assert_eq!(
            c.ajouter(home.path(), "", h),
            Err(ErreurEdition::DossierTropLarge)
        );
        c.ajouter(&dossier(home.path(), "a"), "", h).unwrap();
        assert_eq!(
            c.changer_dossier("~/a", Path::new("/"), h),
            Err(ErreurEdition::DossierTropLarge)
        );
        assert_eq!(
            c.changer_dossier("~/a", home.path(), h),
            Err(ErreurEdition::DossierTropLarge)
        );
        assert_eq!(c.affiches(None).len(), 1);
        assert_eq!(
            ErreurEdition::DossierTropLarge.message(),
            Some("Choisis un dossier de projet, pas la racine ni le dossier personnel.")
        );
    }

    #[test]
    fn ecrire_cree_le_parent_en_0600() {
        let tmp = TempDir::new().unwrap();
        let cible = tmp.path().join("a/b/config.toml");
        avec(&[("/p/a", None)]).ecrire(&cible, None).unwrap();
        assert_eq!(
            fs::metadata(&cible).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(fs::read_to_string(&cible).unwrap().contains("/p/a"));
    }

    #[test]
    fn ecrire_resserre_un_fichier_plus_large() {
        let tmp = TempDir::new().unwrap();
        let cible = tmp.path().join("config.toml");
        fs::write(&cible, "ancien").unwrap();
        fs::set_permissions(&cible, fs::Permissions::from_mode(0o644)).unwrap();
        vide().ecrire(&cible, None).unwrap();
        assert_eq!(
            fs::metadata(&cible).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn ecrire_refuse_un_lien() {
        let tmp = TempDir::new().unwrap();
        let vrai = tmp.path().join("vrai.toml");
        fs::write(&vrai, "intact").unwrap();
        let lien = tmp.path().join("config.toml");
        symlink(&vrai, &lien).unwrap();
        assert_eq!(
            vide().ecrire(&lien, None),
            Err(ErreurEdition::LienSymbolique)
        );
        assert_eq!(fs::read_to_string(&vrai).unwrap(), "intact");
    }
}

//! Enchaînement d'une édition de la liste blanche depuis le menu : lecture,
//! dialogues, opération sur une copie relue, écriture. Voir la spec, 15.2.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::dialogues::{
    INVITE_AJOUT, INVITE_CHANGEMENT, Lanceur, alerter, choisir_dossier, confirmer_retrait,
    demander_nom,
};
use crate::projets::{ConfigEditable, ErreurEdition, nom_dossier};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Ajouter,
    /// Chemin tel qu'écrit dans `config.toml`.
    Renommer(String),
    ChangerDossier(String),
    Retirer(String),
}

/// Ce que les dialogues ont décidé, à appliquer sur la config relue.
enum Decision<'a> {
    Ajouter(PathBuf, String),
    Renommer(&'a str, String),
    ChangerDossier(&'a str, PathBuf),
    Retirer(&'a str),
}

fn signaler(lancer: Lanceur, erreur: ErreurEdition) {
    if let Some(message) = erreur.message() {
        alerter(lancer, message);
    }
}

/// Dialogues, sur la première lecture. `None` : annulé, projet absent ou alerte déjà affichée.
fn decider<'a>(
    action: &'a Action,
    config: &ConfigEditable,
    home: Option<&OsStr>,
    lancer: Lanceur,
) -> Option<Decision<'a>> {
    match action {
        Action::Ajouter => {
            let dossier = choisir_dossier(lancer, INVITE_AJOUT)?;
            if config.contient(&dossier, home) {
                signaler(lancer, ErreurEdition::DejaPresent);
                return None;
            }
            let saisi = demander_nom(lancer, &nom_dossier(&dossier))?;
            Some(Decision::Ajouter(dossier, saisi))
        }
        Action::Renommer(chemin) => {
            let nom = config.nom_affiche(chemin)?;
            let saisi = demander_nom(lancer, &nom)?;
            Some(Decision::Renommer(chemin, saisi))
        }
        Action::ChangerDossier(chemin) => {
            config.nom_affiche(chemin)?;
            let dossier = choisir_dossier(lancer, INVITE_CHANGEMENT)?;
            Some(Decision::ChangerDossier(chemin, dossier))
        }
        Action::Retirer(chemin) => {
            let nom = config.nom_affiche(chemin)?;
            confirmer_retrait(lancer, &nom).then_some(Decision::Retirer(chemin))
        }
    }
}

/// Édite la liste blanche. `true` : `config.toml` a été écrit.
pub fn editer(action: &Action, config: &Path, home: Option<&OsStr>, lancer: Lanceur) -> bool {
    let premiere = match ConfigEditable::lire(config, home) {
        Ok(c) => c,
        Err(e) => {
            signaler(lancer, e);
            return false;
        }
    };
    let Some(decision) = decider(action, &premiere, home, lancer) else {
        return false;
    };
    // Les dialogues ont duré : on applique sur une copie fraîche.
    let resultat = ConfigEditable::lire(config, home).and_then(|mut c| {
        match decision {
            Decision::Ajouter(dossier, saisi) => c.ajouter(&dossier, &saisi, home),
            Decision::Renommer(chemin, saisi) => c.renommer(chemin, &saisi),
            Decision::ChangerDossier(chemin, dossier) => c.changer_dossier(chemin, &dossier, home),
            Decision::Retirer(chemin) => c.retirer(chemin),
        }?;
        c.ecrire(config, home)
    });
    match resultat {
        Ok(()) => {
            log::info!("projets modifiés depuis le menu");
            true
        }
        Err(e) => {
            signaler(lancer, e);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialogues::Reponse;
    use std::collections::VecDeque;
    use std::fs;
    use std::io;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    fn ok(stdout: &str) -> Reponse {
        Reponse {
            code: 0,
            stdout: stdout.to_string(),
        }
    }

    fn annule() -> Reponse {
        Reponse {
            code: 1,
            stdout: String::new(),
        }
    }

    /// Joue `editer` avec des réponses scriptées ; `pendant` s'exécute à chaque appel.
    fn jouer(
        action: &Action,
        config: &Path,
        home: &Path,
        reponses: Vec<Reponse>,
        mut pendant: impl FnMut(usize),
    ) -> (bool, Vec<Vec<String>>) {
        let mut file: VecDeque<Reponse> = reponses.into();
        let mut appels: Vec<Vec<String>> = Vec::new();
        let ecrit = {
            let mut faux = |c: &[&str]| -> io::Result<Reponse> {
                appels.push(c.iter().map(|s| s.to_string()).collect());
                pendant(appels.len());
                Ok(file.pop_front().expect("appel imprévu au lanceur"))
            };
            editer(action, config, Some(home.as_os_str()), &mut faux)
        };
        (ecrit, appels)
    }

    fn cadre() -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let config = tmp.path().join("config.toml");
        (tmp, config)
    }

    fn lire(config: &Path, home: &Path) -> ConfigEditable {
        ConfigEditable::lire(config, Some(home.as_os_str())).unwrap()
    }

    fn dossier(home: &Path, nom: &str) -> PathBuf {
        let d = home.join(nom);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn avec_projet(config: &Path, home: &Path, nom: &str) {
        let d = dossier(home, nom);
        let mut c = ConfigEditable::lire(config, Some(home.as_os_str())).unwrap();
        c.ajouter(&d, "", Some(home.as_os_str())).unwrap();
        c.ecrire(config, Some(home.as_os_str())).unwrap();
    }

    #[test]
    fn ajouter_complet_ecrit_la_config() {
        let (tmp, config) = cadre();
        let home = tmp.path();
        let p = dossier(home, "P");
        let (ecrit, appels) = jouer(
            &Action::Ajouter,
            &config,
            home,
            vec![ok(&format!("{}/\n", p.display())), ok("Joli\n")],
            |_| {},
        );
        assert!(ecrit);
        assert_eq!(appels.len(), 2);
        let c = lire(&config, home);
        assert_eq!(c.nom_affiche("~/P"), Some("Joli".to_string()));
    }

    #[test]
    fn nom_prerempli_vient_du_dossier() {
        let (tmp, config) = cadre();
        let home = tmp.path();
        let p = dossier(home, "Mon Projet");
        let (_, appels) = jouer(
            &Action::Ajouter,
            &config,
            home,
            vec![ok(&format!("{}/\n", p.display())), annule()],
            |_| {},
        );
        assert_eq!(appels[1].last().unwrap(), "Mon Projet");
    }

    #[test]
    fn config_absente_ajout_cree_le_fichier() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();
        let config = home.join("x/y/config.toml");
        let p = dossier(home, "P");
        let (ecrit, _) = jouer(
            &Action::Ajouter,
            &config,
            home,
            vec![ok(&format!("{}/\n", p.display())), ok("\n")],
            |_| {},
        );
        assert!(ecrit);
        let mode = fs::metadata(&config).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn annuler_le_dossier_ne_change_rien() {
        let (tmp, config) = cadre();
        let home = tmp.path();
        avec_projet(&config, home, "A");
        let avant = fs::read(&config).unwrap();
        let (ecrit, appels) = jouer(&Action::Ajouter, &config, home, vec![annule()], |_| {});
        assert!(!ecrit);
        assert_eq!(appels.len(), 1);
        assert_eq!(fs::read(&config).unwrap(), avant);
    }

    #[test]
    fn ajouter_deja_present_alerte_sans_demander_le_nom() {
        let (tmp, config) = cadre();
        let home = tmp.path();
        avec_projet(&config, home, "A");
        let a = home.join("A");
        let avant = fs::read(&config).unwrap();
        let (ecrit, appels) = jouer(
            &Action::Ajouter,
            &config,
            home,
            vec![ok(&format!("{}/\n", a.display())), ok("")],
            |_| {},
        );
        assert!(!ecrit);
        assert_eq!(appels.len(), 2);
        assert_eq!(
            appels[1].last().unwrap(),
            "Ce dossier est déjà dans la liste."
        );
        assert_eq!(fs::read(&config).unwrap(), avant);
    }

    #[test]
    fn config_invalide_alerte_sans_ecrire() {
        let (tmp, config) = cadre();
        fs::write(&config, "ceci n'est pas = = du toml").unwrap();
        let avant = fs::read(&config).unwrap();
        let (ecrit, appels) = jouer(&Action::Ajouter, &config, tmp.path(), vec![ok("")], |_| {});
        assert!(!ecrit);
        assert_eq!(appels.len(), 1);
        assert_eq!(
            appels[0].last().unwrap(),
            ErreurEdition::ConfigInvalide.message().unwrap()
        );
        assert_eq!(fs::read(&config).unwrap(), avant);
    }

    #[test]
    fn renommer_nom_invalide_alerte() {
        let (tmp, config) = cadre();
        let home = tmp.path();
        avec_projet(&config, home, "A");
        let avant = fs::read(&config).unwrap();
        let trop_long = "x".repeat(33);
        let (ecrit, appels) = jouer(
            &Action::Renommer("~/A".into()),
            &config,
            home,
            vec![ok(&format!("{trop_long}\n")), ok("")],
            |_| {},
        );
        assert!(!ecrit);
        assert_eq!(appels.len(), 2);
        assert_eq!(
            appels[1].last().unwrap(),
            ErreurEdition::NomInvalide.message().unwrap()
        );
        assert_eq!(fs::read(&config).unwrap(), avant);
    }

    #[test]
    fn renommer_prerempli_avec_le_nom_affiche_puis_ecrit() {
        let (tmp, config) = cadre();
        let home = tmp.path();
        avec_projet(&config, home, "A");
        let (ecrit, appels) = jouer(
            &Action::Renommer("~/A".into()),
            &config,
            home,
            vec![ok("Beau\n")],
            |_| {},
        );
        assert!(ecrit);
        assert_eq!(appels[0].last().unwrap(), "A");
        assert_eq!(lire(&config, home).nom_affiche("~/A"), Some("Beau".into()));
    }

    #[test]
    fn changer_dossier_garde_l_alias() {
        let (tmp, config) = cadre();
        let home = tmp.path();
        avec_projet(&config, home, "A");
        let b = dossier(home, "B");
        let mut c = lire(&config, home);
        c.renommer("~/A", "Joli").unwrap();
        c.ecrire(&config, Some(home.as_os_str())).unwrap();
        let (ecrit, _) = jouer(
            &Action::ChangerDossier("~/A".into()),
            &config,
            home,
            vec![ok(&format!("{}/\n", b.display()))],
            |_| {},
        );
        assert!(ecrit);
        let c = lire(&config, home);
        assert_eq!(c.nom_affiche("~/B"), Some("Joli".into()));
        assert_eq!(c.nom_affiche("~/A"), None);
    }

    #[test]
    fn changer_dossier_vers_lui_meme_alerte() {
        let (tmp, config) = cadre();
        let home = tmp.path();
        avec_projet(&config, home, "A");
        let a = home.join("A");
        let (ecrit, appels) = jouer(
            &Action::ChangerDossier("~/A".into()),
            &config,
            home,
            vec![ok(&format!("{}/\n", a.display())), ok("")],
            |_| {},
        );
        assert!(!ecrit);
        assert_eq!(
            appels[1].last().unwrap(),
            "Ce dossier est déjà dans la liste."
        );
    }

    #[test]
    fn retirer_confirme() {
        let (tmp, config) = cadre();
        let home = tmp.path();
        avec_projet(&config, home, "A");
        let (ecrit, appels) = jouer(
            &Action::Retirer("~/A".into()),
            &config,
            home,
            vec![ok("")],
            |_| {},
        );
        assert!(ecrit);
        assert_eq!(
            appels[0].last().unwrap(),
            "Retirer « A » des projets affichés ?"
        );
        assert_eq!(lire(&config, home).nom_affiche("~/A"), None);
    }

    #[test]
    fn retirer_annule() {
        let (tmp, config) = cadre();
        let home = tmp.path();
        avec_projet(&config, home, "A");
        let avant = fs::read(&config).unwrap();
        let (ecrit, _) = jouer(
            &Action::Retirer("~/A".into()),
            &config,
            home,
            vec![annule()],
            |_| {},
        );
        assert!(!ecrit);
        assert_eq!(fs::read(&config).unwrap(), avant);
    }

    #[test]
    fn projet_disparu_ne_fait_rien() {
        let (tmp, config) = cadre();
        let home = tmp.path();
        avec_projet(&config, home, "A");
        let avant = fs::read(&config).unwrap();
        for action in [
            Action::Renommer("~/absent".into()),
            Action::ChangerDossier("~/absent".into()),
            Action::Retirer("~/absent".into()),
        ] {
            let (ecrit, appels) = jouer(&action, &config, home, vec![], |_| {});
            assert!(!ecrit);
            assert!(appels.is_empty());
        }
        assert_eq!(fs::read(&config).unwrap(), avant);
    }

    #[test]
    fn projet_disparu_pendant_le_dialogue_sans_alerte() {
        let (tmp, config) = cadre();
        let home = tmp.path();
        avec_projet(&config, home, "A");
        let config2 = config.clone();
        let (ecrit, appels) = jouer(
            &Action::Renommer("~/A".into()),
            &config,
            home,
            vec![ok("Joli\n")],
            |_| fs::write(&config2, "[projets]\nautorises = []\n").unwrap(),
        );
        assert!(!ecrit);
        assert_eq!(appels.len(), 1);
    }

    #[test]
    fn modification_concurrente_gardee() {
        let (tmp, config) = cadre();
        let home = tmp.path();
        avec_projet(&config, home, "A");
        let b = dossier(home, "B");
        let p = dossier(home, "P");
        let config2 = config.clone();
        let home2 = home.to_path_buf();
        let (ecrit, _) = jouer(
            &Action::Ajouter,
            &config,
            home,
            vec![ok(&format!("{}/\n", p.display())), ok("Joli\n")],
            |n| {
                if n == 2 {
                    let mut c = ConfigEditable::lire(&config2, Some(home2.as_os_str())).unwrap();
                    c.ajouter(&b, "", Some(home2.as_os_str())).unwrap();
                    c.ecrire(&config2, Some(home2.as_os_str())).unwrap();
                }
            },
        );
        assert!(ecrit);
        let c = lire(&config, home);
        assert!(c.nom_affiche("~/A").is_some());
        assert!(c.nom_affiche("~/B").is_some());
        assert_eq!(c.nom_affiche("~/P"), Some("Joli".into()));
    }
}

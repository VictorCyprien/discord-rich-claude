//! Dialogues natifs via `/usr/bin/osascript`. Les textes saisis ou affichés
//! passent en arguments (`on run argv`), jamais dans le source AppleScript.

use std::io;
use std::path::PathBuf;

pub const OSASCRIPT: &str = "/usr/bin/osascript";
pub const TITRE: &str = "Clauderune";
pub const INVITE_AJOUT: &str = "Choisir le dossier du projet";
pub const INVITE_CHANGEMENT: &str = "Choisir le nouveau dossier du projet";
pub const QUESTION_NOM: &str = "Nom affiché sur Discord :";

#[derive(Clone, Debug)]
pub struct Reponse {
    pub code: i32,
    pub stdout: String,
}

pub type Lanceur<'a> = &'a mut dyn FnMut(&[&str]) -> io::Result<Reponse>;

pub fn lanceur_reel(commande: &[&str]) -> io::Result<Reponse> {
    let sortie = std::process::Command::new(commande[0])
        .args(&commande[1..])
        .output()?;
    Ok(Reponse {
        code: sortie.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&sortie.stdout).into_owned(),
    })
}

/// Lance un script (une ligne par `-e`) avec ses arguments. `None` : annulé
/// ou échec ; sinon la sortie, privée d'un seul `\n` final.
fn executer(lancer: Lanceur, lignes: &[&str], arguments: &[&str]) -> Option<String> {
    // `getopt` d'osascript lirait un premier argument en `-` comme une option.
    if arguments.first().is_some_and(|a| a.starts_with('-')) {
        return None;
    }
    let mut commande = vec![OSASCRIPT];
    for ligne in lignes {
        commande.push("-e");
        commande.push(ligne);
    }
    commande.extend_from_slice(arguments);
    let reponse = match lancer(&commande) {
        Ok(r) => r,
        Err(e) => {
            log::warn!("osascript inutilisable : {:?}", e.kind());
            return None;
        }
    };
    if reponse.code != 0 {
        // Le code 1 est l'annulation normale (-128) : muet.
        if reponse.code != 1 {
            log::warn!("osascript a échoué (code {})", reponse.code);
        }
        return None;
    }
    let mut sortie = reponse.stdout;
    if sortie.ends_with('\n') {
        sortie.pop();
    }
    Some(sortie)
}

pub fn choisir_dossier(lancer: Lanceur, invite: &str) -> Option<PathBuf> {
    let sortie = executer(
        lancer,
        &[
            "on run argv",
            "activate",
            "POSIX path of (choose folder with prompt (item 1 of argv))",
            "end run",
        ],
        &[invite],
    )?;
    (!sortie.is_empty()).then(|| PathBuf::from(sortie))
}

pub fn demander_nom(lancer: Lanceur, defaut: &str) -> Option<String> {
    executer(
        lancer,
        &[
            "on run argv",
            "activate",
            "text returned of (display dialog (item 1 of argv) default answer (item 2 of argv) with title \"Clauderune\" buttons {\"Annuler\", \"OK\"} default button \"OK\" cancel button \"Annuler\")",
            "end run",
        ],
        &[QUESTION_NOM, defaut],
    )
}

pub fn confirmer_retrait(lancer: Lanceur, nom: &str) -> bool {
    let question = format!("Retirer « {nom} » des projets affichés ?");
    executer(
        lancer,
        &[
            "on run argv",
            "activate",
            "display dialog (item 1 of argv) with title \"Clauderune\" buttons {\"Annuler\", \"Retirer\"} default button \"Annuler\" cancel button \"Annuler\"",
            "end run",
        ],
        &[&question],
    )
    .is_some()
}

pub fn alerter(lancer: Lanceur, message: &str) {
    executer(
        lancer,
        &[
            "on run argv",
            "activate",
            "display alert \"Clauderune\" message (item 1 of argv) as warning buttons {\"OK\"} default button \"OK\"",
            "end run",
        ],
        &[message],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    type Appels = Vec<Vec<String>>;

    /// Lance `f` avec un faux lanceur qui rend `(code, stdout)` ; rend les commandes vues.
    fn avec_faux<R>(code: i32, stdout: &str, f: impl FnOnce(Lanceur) -> R) -> (R, Appels) {
        let mut appels: Appels = Vec::new();
        let r = {
            let mut faux = |c: &[&str]| -> io::Result<Reponse> {
                appels.push(c.iter().map(|s| s.to_string()).collect());
                Ok(Reponse {
                    code,
                    stdout: stdout.to_string(),
                })
            };
            f(&mut faux)
        };
        (r, appels)
    }

    /// Premier argument après les scripts : le premier élément qui ne suit pas un `-e`.
    fn premier_argument(commande: &[String]) -> &str {
        let mut i = 1;
        while commande[i] == "-e" {
            i += 2;
        }
        &commande[i]
    }

    fn dernier_argument(commande: &[String]) -> &str {
        commande.last().unwrap()
    }

    fn lignes_e(commande: &[String]) -> Vec<&str> {
        commande
            .windows(2)
            .filter(|w| w[0] == "-e")
            .map(|w| w[1].as_str())
            .collect()
    }

    const PIEGE: &str = "x\" & (do shell script \"id\") & \"";

    #[test]
    fn commande_commence_par_osascript_absolu() {
        let (_, appels) = avec_faux(0, "", |l| choisir_dossier(l, INVITE_AJOUT));
        assert_eq!(appels[0][0], "/usr/bin/osascript");
    }

    #[test]
    fn aucun_texte_saisi_dans_les_lignes_e() {
        let (_, appels) = avec_faux(0, "", |l| demander_nom(l, PIEGE));
        assert_eq!(dernier_argument(&appels[0]), PIEGE);
        assert!(
            lignes_e(&appels[0])
                .iter()
                .all(|l| !l.contains("do shell script"))
        );

        let (_, appels) = avec_faux(0, "", |l| confirmer_retrait(l, PIEGE));
        assert!(dernier_argument(&appels[0]).contains(PIEGE));
        assert!(
            lignes_e(&appels[0])
                .iter()
                .all(|l| !l.contains("do shell script"))
        );
    }

    #[test]
    fn premier_argument_fixe() {
        let (_, a) = avec_faux(0, "/tmp/\n", |l| choisir_dossier(l, INVITE_CHANGEMENT));
        assert_eq!(premier_argument(&a[0]), INVITE_CHANGEMENT);
        let (_, a) = avec_faux(0, "", |l| demander_nom(l, "-x"));
        assert_eq!(premier_argument(&a[0]), QUESTION_NOM);
        let (_, a) = avec_faux(0, "", |l| confirmer_retrait(l, "-x"));
        assert!(premier_argument(&a[0]).starts_with("Retirer « "));
        let (_, a) = avec_faux(0, "", |l| alerter(l, "Ce dossier est déjà dans la liste."));
        assert_eq!(
            premier_argument(&a[0]),
            "Ce dossier est déjà dans la liste."
        );
    }

    #[test]
    fn demander_nom_retire_un_seul_saut_de_ligne() {
        let (nom, _) = avec_faux(0, "  Joli \n", |l| demander_nom(l, "x"));
        assert_eq!(nom, Some("  Joli ".to_string()));
        let (nom, _) = avec_faux(0, "Joli\n\n", |l| demander_nom(l, "x"));
        assert_eq!(nom, Some("Joli\n".to_string()));
    }

    #[test]
    fn annulation_code_1() {
        let (d, _) = avec_faux(1, "/tmp/\n", |l| choisir_dossier(l, INVITE_AJOUT));
        assert_eq!(d, None);
        let (n, _) = avec_faux(1, "Joli\n", |l| demander_nom(l, "x"));
        assert_eq!(n, None);
        let (c, _) = avec_faux(1, "", |l| confirmer_retrait(l, "x"));
        assert!(!c);
    }

    #[test]
    fn erreur_d_entree_sortie_vaut_annulation() {
        let mut faux = |_: &[&str]| -> io::Result<Reponse> { Err(io::Error::other("absent")) };
        assert_eq!(choisir_dossier(&mut faux, INVITE_AJOUT), None);
        assert!(!confirmer_retrait(&mut faux, "x"));
    }

    #[test]
    fn premier_argument_commencant_par_tiret_ne_lance_rien() {
        let (_, appels) = avec_faux(0, "", |l| {
            alerter(l, "-eproperty x : (do shell script \"id\")")
        });
        assert!(appels.is_empty());
        let (d, appels) = avec_faux(0, "/tmp/\n", |l| choisir_dossier(l, "-x"));
        assert_eq!(d, None);
        assert!(appels.is_empty());
    }

    #[test]
    fn code_inattendu_vaut_annulation_sans_paniquer() {
        let (d, _) = avec_faux(-1, "/tmp/\n", |l| choisir_dossier(l, INVITE_AJOUT));
        assert_eq!(d, None);
        let (c, _) = avec_faux(2, "", |l| confirmer_retrait(l, "x"));
        assert!(!c);
    }

    #[test]
    fn choisir_dossier_rend_le_chemin() {
        let (d, _) = avec_faux(0, "/tmp/a b/\n", |l| choisir_dossier(l, INVITE_AJOUT));
        assert_eq!(d, Some(PathBuf::from("/tmp/a b/")));
    }

    #[test]
    fn choisir_dossier_vide_est_annule() {
        let (d, _) = avec_faux(0, "\n", |l| choisir_dossier(l, INVITE_AJOUT));
        assert_eq!(d, None);
    }
}

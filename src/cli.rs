//! Ligne de commande. Voir la spec, sections 8 et 14.4.
//!
//! Chaque commande existe en deux versions : celle qui parle au terminal, et une variante
//! `_vers` qui écrit sur des flux passés en paramètre (les tests y lisent la sortie).

use std::io::{self, BufRead, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::bundle::{
    EtatLien, NOM_ANCIEN_BUNDLE, binaire_du_bundle, desinstaller_bundle, etat_lien,
    installer_bundle, poser_lien, retirer_ancien_bundle, retirer_lien,
};
use crate::chemins::{COMMANDE_HOOK, Chemins};
use crate::config::{ConfigSurveillee, creer_config_par_defaut};
use crate::daemon::{Daemon, EnMarche, Mode, demarrer, horloge_systeme};
use crate::installation::{
    ErreurLaunchctl, Executeur, Resultat, ajouter_hooks, desinstaller_launch_agent, executeur_reel,
    generer_plist, installer_launch_agent, modifier_settings, retirer_hooks,
};
use crate::journal::{configurer_logs, installer_hook_panique};
use crate::presence::{
    Fabrique, INTERVALLE_MIN_S, Presence, REESSAI_S, fabrique_reelle, horloge_monotone,
};
use crate::serveur::{ErreurDemarrage, interroger_etat};
use crate::sessions::pid_vivant;
use crate::tokens::SuiviTokens;

pub const USAGE: &str = "usage : discord-rich-claude <run|install|uninstall|status>";
const DELAI_STATUT: Duration = Duration::from_secs(2);

fn ecrire(flux: &mut dyn Write, texte: &str) {
    let _ = writeln!(flux, "{texte}");
}

/// Lance le daemon, puis la fonction de menu (qui ne rend la main que dans les tests).
/// Rend 1 sans jamais appeler `menu` si le démarrage échoue.
pub fn lancer(chemins: &Chemins, menu: impl FnOnce(Arc<Daemon>, EnMarche) -> i32) -> i32 {
    lancer_vers(chemins, fabrique_reelle(), menu, &mut io::stderr())
}

pub fn lancer_vers(
    chemins: &Chemins,
    fabrique: Fabrique,
    menu: impl FnOnce(Arc<Daemon>, EnMarche) -> i32,
    erreur: &mut dyn Write,
) -> i32 {
    let daemon = Daemon::new(
        ConfigSurveillee::new(chemins.config.clone()),
        Presence::new(fabrique, INTERVALLE_MIN_S, REESSAI_S, horloge_monotone()),
        SuiviTokens::new(chemins.projets_claude.clone()),
        chemins.projets_claude.clone(),
        Box::new(pid_vivant),
        horloge_systeme(),
    );
    // Le socket est lié ici : seulement ensuite, l'interface.
    match demarrer(&daemon, &chemins.socket) {
        Ok(en_marche) => menu(daemon, en_marche),
        Err(ErreurDemarrage::DejaLance(_)) => {
            log::error!("un daemon tourne déjà sur {}", chemins.socket.display());
            ecrire(
                erreur,
                &format!(
                    "discord-rich-claude : un daemon tourne déjà sur {}",
                    chemins.socket.display()
                ),
            );
            1
        }
        Err(ErreurDemarrage::Io(e)) => {
            // Une ligne dans le journal et sur stderr, jamais de trace : sous le LaunchAgent,
            // stderr part dans un fichier.
            log::error!(
                "démarrage impossible ({:?}), socket {}",
                e.kind(),
                chemins.socket.display()
            );
            ecrire(
                erreur,
                &format!(
                    "discord-rich-claude : démarrage impossible ({:?}), socket {}",
                    e.kind(),
                    chemins.socket.display()
                ),
            );
            1
        }
    }
}

/// Confirmation du diff : `o` explicite seulement. Fin de fichier ou erreur de lecture = refus.
pub fn confirmer_depuis(
    entree: &mut dyn BufRead,
    sortie: &mut dyn Write,
    settings: &Path,
    diff: &str,
) -> bool {
    ecrire(sortie, diff);
    let _ = write!(
        sortie,
        "Appliquer ces changements à {} ? [o/N] ",
        settings.display()
    );
    let _ = sortie.flush();
    let mut reponse = String::new();
    match entree.read_line(&mut reponse) {
        Ok(0) | Err(_) => {
            ecrire(sortie, "");
            false
        }
        Ok(_) => reponse.trim().to_lowercase() == "o",
    }
}

/// Confirmation de production : lit le vrai stdin. Jamais contournée.
fn confirmer_terminal(settings: &Path, diff: &str) -> bool {
    confirmer_depuis(&mut io::stdin().lock(), &mut io::stdout(), settings, diff)
}

fn maintenant() -> chrono::NaiveDateTime {
    chrono::Local::now().naive_local()
}

fn refus_sudo(erreur: &mut dyn Write) -> i32 {
    ecrire(erreur, "ne pas lancer avec sudo : relance sans sudo");
    1
}

pub fn installer_avec(
    chemins: &Chemins,
    binaire_courant: &Path,
    euid: u32,
    executer: Executeur,
    confirmer: impl FnOnce(&str) -> bool,
) -> i32 {
    installer_vers(
        chemins,
        binaire_courant,
        euid,
        executer,
        confirmer,
        &mut io::stdout(),
        &mut io::stderr(),
    )
}

pub fn installer_vers(
    chemins: &Chemins,
    binaire_courant: &Path,
    euid: u32,
    executer: Executeur,
    confirmer: impl FnOnce(&str) -> bool,
    sortie: &mut dyn Write,
    erreur: &mut dyn Write,
) -> i32 {
    if euid == 0 {
        return refus_sudo(erreur);
    }
    // Avant toute autre étape : si une autre entrée occupe le lien, rien n'est modifié.
    let binaire_bundle = binaire_du_bundle(&chemins.applications);
    let etat_du_lien = match etat_lien(&chemins.lien_commande, &chemins.applications) {
        Ok(EtatLien::Occupe) => {
            ecrire(
                erreur,
                &format!(
                    "{} existe déjà (version Python ?) : lance d'abord `uv tool uninstall discord-rich-claude`, puis relance `discord-rich-claude install`",
                    chemins.lien_commande.display()
                ),
            );
            return 1;
        }
        Ok(etat) => etat,
        Err(e) => {
            ecrire(
                erreur,
                &format!("  {:?} : {}", e.kind(), chemins.lien_commande.display()),
            );
            return 1;
        }
    };
    ecrire(sortie, "1/5 Hooks Claude Code");
    let resultat = match modifier_settings(
        &chemins.settings_claude,
        |s| ajouter_hooks(s, COMMANDE_HOOK),
        confirmer,
        maintenant,
    ) {
        Ok(r) => r,
        Err(e) => {
            ecrire(erreur, &format!("settings.json non modifié : {e}"));
            return 1;
        }
    };
    if resultat == Resultat::Refuse {
        ecrire(sortie, "  installation annulée, rien n'a été modifié");
        return 1;
    }
    ecrire(sortie, &format!("  {resultat}"));

    ecrire(sortie, "2/5 Configuration");
    match creer_config_par_defaut(&chemins.config) {
        Ok(true) => ecrire(
            sortie,
            &format!(
                "  créée : {}. Renseigne client_id et projets.autorises.",
                chemins.config.display()
            ),
        ),
        Ok(false) => ecrire(
            sortie,
            &format!("  déjà présente : {}", chemins.config.display()),
        ),
        Err(e) => {
            ecrire(
                erreur,
                &format!("  {:?} : {}", e.kind(), chemins.config.display()),
            );
            return 1;
        }
    }

    ecrire(sortie, "3/5 Application");
    let executable = match installer_bundle(&chemins.applications, binaire_courant, &mut *executer)
    {
        Ok(e) => e,
        Err(e) => {
            ecrire(
                erreur,
                &format!("  {e} : relance `discord-rich-claude install`"),
            );
            return 1;
        }
    };
    ecrire(sortie, &format!("  exécutable : {}", executable.display()));

    ecrire(sortie, "4/5 LaunchAgent");
    if let Err(e) = std::fs::create_dir_all(&chemins.dossier_logs) {
        ecrire(
            erreur,
            &format!("  {:?} : {}", e.kind(), chemins.dossier_logs.display()),
        );
        return 1;
    }
    let plist = generer_plist(
        &binaire_du_bundle(&chemins.applications),
        &chemins.dossier_logs,
    );
    match installer_launch_agent(&chemins.launch_agent, &plist, euid, executer, &mut |d| {
        std::thread::sleep(d)
    }) {
        Ok(()) => {}
        Err(ErreurLaunchctl::Io(e)) => {
            ecrire(
                erreur,
                &format!("  {:?} : {}", e.kind(), chemins.launch_agent.display()),
            );
            return 1;
        }
        Err(ErreurLaunchctl::Echec { code, stderr }) => {
            ecrire(
                erreur,
                &format!(
                    "  launchctl a échoué ({code}) : {stderr} — relance `discord-rich-claude install`"
                ),
            );
            return 1;
        }
    }
    ecrire(sortie, "  LaunchAgent chargé");

    ecrire(sortie, "5/5 Commande");
    if etat_du_lien != EtatLien::Correct {
        if let Err(e) = poser_lien(&chemins.lien_commande, &chemins.applications) {
            ecrire(
                erreur,
                &format!("  {:?} : {}", e.kind(), chemins.lien_commande.display()),
            );
            return 1;
        }
        let verbe = if etat_du_lien == EtatLien::Ancien {
            "lien mis à jour"
        } else {
            "lien créé"
        };
        ecrire(
            sortie,
            &format!(
                "  {verbe} : {} -> {}",
                chemins.lien_commande.display(),
                binaire_bundle.display()
            ),
        );
    } else {
        ecrire(
            sortie,
            &format!("  déjà présent : {}", chemins.lien_commande.display()),
        );
    }
    // Seulement après le LaunchAgent et le lien : tant que l'un échoue, l'ancien bundle garde la commande vivante.
    let ancien = chemins.applications.join(NOM_ANCIEN_BUNDLE);
    match retirer_ancien_bundle(&chemins.applications) {
        Ok(true) => ecrire(
            sortie,
            &format!("  ancienne app retirée : {}", ancien.display()),
        ),
        Ok(false) => {}
        Err(_) => ecrire(
            erreur,
            &format!("  ancienne app laissée en place : {}", ancien.display()),
        ),
    }
    0
}

pub fn desinstaller_avec(
    chemins: &Chemins,
    euid: u32,
    executer: Executeur,
    confirmer: impl FnOnce(&str) -> bool,
) -> i32 {
    desinstaller_vers(
        chemins,
        euid,
        executer,
        confirmer,
        &mut io::stdout(),
        &mut io::stderr(),
    )
}

pub fn desinstaller_vers(
    chemins: &Chemins,
    euid: u32,
    executer: Executeur,
    confirmer: impl FnOnce(&str) -> bool,
    sortie: &mut dyn Write,
    erreur: &mut dyn Write,
) -> i32 {
    if euid == 0 {
        return refus_sudo(erreur);
    }
    match modifier_settings(
        &chemins.settings_claude,
        retirer_hooks,
        confirmer,
        maintenant,
    ) {
        Ok(Resultat::Refuse) => {
            ecrire(sortie, "désinstallation annulée");
            return 1;
        }
        Ok(_) => {}
        Err(e) => {
            ecrire(erreur, &format!("settings.json non modifié : {e}"));
            return 1;
        }
    }
    if let Err(e) = desinstaller_launch_agent(&chemins.launch_agent, euid, &mut *executer) {
        ecrire(
            erreur,
            &format!("  {:?} : {}", e.kind(), chemins.launch_agent.display()),
        );
        return 1;
    }
    if let Err(e) = desinstaller_bundle(&chemins.applications) {
        ecrire(
            erreur,
            &format!("  {:?} : {}", e.kind(), chemins.applications.display()),
        );
        return 1;
    }
    match etat_lien(&chemins.lien_commande, &chemins.applications) {
        Ok(EtatLien::Absent) => {}
        Ok(EtatLien::Correct | EtatLien::Ancien) => {
            if let Err(e) = retirer_lien(&chemins.lien_commande, &chemins.applications) {
                ecrire(
                    erreur,
                    &format!("  {:?} : {}", e.kind(), chemins.lien_commande.display()),
                );
                return 1;
            }
        }
        Ok(EtatLien::Occupe) => ecrire(
            erreur,
            &format!(
                "{} n'est pas notre lien : laissé en place",
                chemins.lien_commande.display()
            ),
        ),
        Err(e) => {
            ecrire(
                erreur,
                &format!("  {:?} : {}", e.kind(), chemins.lien_commande.display()),
            );
            return 1;
        }
    }
    ecrire(
        sortie,
        &format!(
            "Désinstallé. Config conservée : {}",
            chemins.config.display()
        ),
    );
    0
}

pub fn statut(chemins: &Chemins) -> i32 {
    statut_vers(chemins, &mut io::stdout())
}

pub fn statut_vers(chemins: &Chemins, sortie: &mut dyn Write) -> i32 {
    let Some(etat) = interroger_etat(&chemins.socket, DELAI_STATUT) else {
        ecrire(sortie, "daemon : injoignable");
        return 1;
    };
    ecrire(sortie, "daemon : actif");
    let connecte = etat.get("discord").and_then(|v| v.as_bool()) == Some(true);
    ecrire(
        sortie,
        &format!(
            "discord : {}",
            if connecte {
                "connecté"
            } else {
                "non connecté"
            }
        ),
    );
    let sessions = etat.get("sessions").and_then(|v| v.as_i64()).unwrap_or(0);
    ecrire(sortie, &format!("sessions : {sessions}"));
    let mode = [Mode::Allume, Mode::Pause, Mode::Arrete]
        .into_iter()
        .find(|m| etat.get("etat").and_then(|v| v.as_str()) == Some(m.code()));
    if let Some(mode) = mode {
        ecrire(sortie, &format!("état : {}", mode.libelle()));
    }
    0
}

/// Avec ou sans la feature `menu`, `lancer_menu` ne rend jamais la main.
fn menu_reel(daemon: Arc<Daemon>, en_marche: EnMarche) -> i32 {
    crate::menu::lancer_menu(daemon, en_marche)
}

/// Commande demandée par la ligne de commande.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Commande {
    Run,
    Install,
    Uninstall,
    Status,
    Inconnue,
}

/// Analyse pure des arguments (après le nom du programme). Aucun argument, ou seulement
/// des arguments `-psn_…` (ancien LaunchServices), équivaut à `run` : c'est ce que lance
/// l'ouverture de l'app depuis Spotlight ou le Launchpad.
pub fn commande_depuis(args: &[String]) -> Commande {
    if args.iter().all(|a| a.starts_with("-psn_")) {
        return Commande::Run;
    }
    match args.first().map(String::as_str) {
        Some("run") => Commande::Run,
        Some("install") => Commande::Install,
        Some("uninstall") => Commande::Uninstall,
        Some("status") => Commande::Status,
        _ => Commande::Inconnue,
    }
}

/// Point d'entrée : `args` sont les arguments après le nom du programme.
pub fn principal(args: Vec<String>) -> i32 {
    principal_vers(args, &mut io::stderr())
}

/// `principal`, avec le flux d'erreur en paramètre. Une commande inconnue rend 2 avant tout
/// accès aux vrais chemins : c'est la seule que les tests peuvent lancer.
pub fn principal_vers(args: Vec<String>, erreur: &mut dyn Write) -> i32 {
    let commande = commande_depuis(&args);
    if commande == Commande::Inconnue {
        ecrire(erreur, USAGE);
        return 2;
    }
    let Some(chemins) = Chemins::reels() else {
        ecrire(erreur, "discord-rich-claude : HOME introuvable");
        return 1;
    };
    // SAFETY : `geteuid` n'a aucun effet de bord.
    let euid = unsafe { libc::geteuid() };
    match commande {
        Commande::Run => {
            installer_hook_panique();
            configurer_logs(&chemins.dossier_logs);
            lancer(&chemins, menu_reel)
        }
        Commande::Install => match std::env::current_exe() {
            Ok(binaire) => installer_avec(&chemins, &binaire, euid, &mut executeur_reel, |diff| {
                confirmer_terminal(&chemins.settings_claude, diff)
            }),
            Err(e) => {
                ecrire(erreur, &format!("exécutable introuvable ({:?})", e.kind()));
                1
            }
        },
        Commande::Uninstall => desinstaller_avec(&chemins, euid, &mut executeur_reel, |diff| {
            confirmer_terminal(&chemins.settings_claude, diff)
        }),
        _ => statut(&chemins),
    }
}

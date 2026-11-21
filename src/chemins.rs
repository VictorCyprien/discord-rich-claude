//! Emplacements sur disque et commande de hook. Voir la spec, sections 7.5 et 8.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io;
use std::path::{Component, Path, PathBuf};

pub const LABEL: &str = "local.discord-rich-claude";

/// `$HOME` et `$PPID` sont développés par le shell du hook : `$PPID` y désigne le
/// processus Claude Code, que le daemon surveille pour purger les sessions mortes.
pub const COMMANDE_HOOK: &str = "curl -s --max-time 1 --unix-socket \"$HOME/.local/state/discord-rich-claude/daemon.sock\" -H \"X-Claude-Pid: $PPID\" --data-binary @- http://localhost/event >/dev/null 2>&1 || true";

/// Nombre maximal de liens symboliques suivis avant de conclure à une boucle.
const LIENS_MAX: u32 = 40;

#[derive(Clone, Debug, PartialEq)]
pub struct Chemins {
    pub home: PathBuf,
    pub dossier_etat: PathBuf,
    pub socket: PathBuf,
    pub config: PathBuf,
    pub dossier_logs: PathBuf,
    pub projets_claude: PathBuf,
    pub settings_claude: PathBuf,
    pub launch_agent: PathBuf,
    pub applications: PathBuf,
    /// Lien `~/.local/bin/discord-rich-claude` vers le binaire du bundle.
    pub lien_commande: PathBuf,
}

impl Chemins {
    pub fn depuis_home(home: &Path) -> Chemins {
        let dossier_etat = home
            .join(".local")
            .join("state")
            .join("discord-rich-claude");
        Chemins {
            home: home.to_path_buf(),
            socket: dossier_etat.join("daemon.sock"),
            dossier_etat,
            config: home
                .join(".config")
                .join("discord-rich-claude")
                .join("config.toml"),
            dossier_logs: home
                .join("Library")
                .join("Logs")
                .join("discord-rich-claude"),
            projets_claude: home.join(".claude").join("projects"),
            settings_claude: home.join(".claude").join("settings.json"),
            launch_agent: home
                .join("Library")
                .join("LaunchAgents")
                .join(format!("{LABEL}.plist")),
            applications: home.join("Applications"),
            lien_commande: home.join(".local").join("bin").join("discord-rich-claude"),
        }
    }

    /// Les emplacements réels, d'après `HOME`. `None` si `HOME` est absent ou vide.
    pub fn reels() -> Option<Chemins> {
        let home = std::env::var_os("HOME")?;
        if home.is_empty() {
            return None;
        }
        Some(Chemins::depuis_home(Path::new(&home)))
    }
}

/// Équivalent de `Path.resolve(strict)` (donc de `os.path.realpath`) : suit les
/// liens composant par composant, applique `..` après la résolution du lien qui
/// précède, et rend une erreur au-delà de 40 liens suivis. En mode non strict,
/// la partie qui n'existe pas est recollée telle quelle.
pub fn resoudre(p: &Path, strict: bool) -> io::Result<PathBuf> {
    let absolu = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()?.join(p)
    };
    let mut resultat = PathBuf::from("/");
    let mut restant: VecDeque<OsString> = noms(&absolu).collect();
    let mut liens = 0u32;

    while let Some(nom) = restant.pop_front() {
        if nom == ".." {
            resultat.pop();
            continue;
        }
        let candidat = resultat.join(&nom);
        match std::fs::symlink_metadata(&candidat) {
            Ok(meta) if meta.file_type().is_symlink() => {
                liens += 1;
                if liens > LIENS_MAX {
                    return Err(io::Error::other("trop de liens symboliques"));
                }
                let cible = std::fs::read_link(&candidat)?;
                if cible.is_absolute() {
                    resultat = PathBuf::from("/");
                }
                for n in noms(&cible).collect::<Vec<_>>().into_iter().rev() {
                    restant.push_front(n);
                }
            }
            Ok(_) => resultat = candidat,
            Err(e) => {
                if strict {
                    return Err(e);
                }
                resultat = candidat;
            }
        }
    }
    Ok(resultat)
}

/// Les composants nommés d'un chemin (`..` compris), sans `/` ni `.`.
fn noms(p: &Path) -> impl Iterator<Item = OsString> + '_ {
    p.components().filter_map(|c| match c {
        Component::Normal(n) => Some(n.to_os_string()),
        Component::ParentDir => Some(OsString::from("..")),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn racine() -> (tempfile::TempDir, PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let r = t.path().canonicalize().unwrap();
        (t, r)
    }

    #[test]
    fn chemins_depuis_home() {
        let c = Chemins::depuis_home(Path::new("/h"));
        assert_eq!(
            c.dossier_etat,
            Path::new("/h/.local/state/discord-rich-claude")
        );
        assert_eq!(
            c.socket,
            Path::new("/h/.local/state/discord-rich-claude/daemon.sock")
        );
        assert_eq!(
            c.config,
            Path::new("/h/.config/discord-rich-claude/config.toml")
        );
        assert_eq!(
            c.dossier_logs,
            Path::new("/h/Library/Logs/discord-rich-claude")
        );
        assert_eq!(c.projets_claude, Path::new("/h/.claude/projects"));
        assert_eq!(c.settings_claude, Path::new("/h/.claude/settings.json"));
        assert_eq!(
            c.launch_agent,
            Path::new("/h/Library/LaunchAgents/local.discord-rich-claude.plist")
        );
        assert_eq!(c.applications, Path::new("/h/Applications"));
        assert_eq!(
            c.lien_commande,
            Path::new("/h/.local/bin/discord-rich-claude")
        );
    }

    #[test]
    fn resoudre_lien_absolu() {
        let (_t, r) = racine();
        std::fs::create_dir(r.join("vrai")).unwrap();
        symlink(r.join("vrai"), r.join("lien")).unwrap();
        assert_eq!(resoudre(&r.join("lien"), true).unwrap(), r.join("vrai"));
    }

    #[test]
    fn resoudre_lien_relatif() {
        let (_t, r) = racine();
        std::fs::create_dir(r.join("vrai")).unwrap();
        symlink("vrai", r.join("lien")).unwrap();
        assert_eq!(resoudre(&r.join("lien"), true).unwrap(), r.join("vrai"));
    }

    #[test]
    fn resoudre_parent_apres_un_lien() {
        let (_t, r) = racine();
        std::fs::create_dir_all(r.join("a/b")).unwrap();
        std::fs::create_dir(r.join("c")).unwrap();
        symlink(r.join("a/b"), r.join("c/lien")).unwrap();
        // `lien/..` vaut `a`, pas `c`.
        assert_eq!(resoudre(&r.join("c/lien/.."), true).unwrap(), r.join("a"));
    }

    #[test]
    fn resoudre_absent_non_strict_recolle() {
        let (_t, r) = racine();
        assert_eq!(
            resoudre(&r.join("absent/sous/fichier"), false).unwrap(),
            r.join("absent/sous/fichier")
        );
    }

    #[test]
    fn resoudre_absent_strict_not_found() {
        let (_t, r) = racine();
        let e = resoudre(&r.join("absent"), true).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn resoudre_boucle_de_liens() {
        let (_t, r) = racine();
        symlink(r.join("b"), r.join("a")).unwrap();
        symlink(r.join("a"), r.join("b")).unwrap();
        assert!(resoudre(&r.join("a"), false).is_err());
        assert!(resoudre(&r.join("a"), true).is_err());
    }

    #[test]
    fn resoudre_chemin_relatif_depuis_le_repertoire_courant() {
        let r = resoudre(Path::new("."), true).unwrap();
        assert_eq!(r, std::env::current_dir().unwrap().canonicalize().unwrap());
    }
}

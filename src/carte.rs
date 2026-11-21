//! Construction de la carte Discord. Fonctions pures, sauf la résolution des
//! dossiers de projet, mise en cache. Voir la spec, section 5.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use crate::chemins::resoudre;
use crate::config::Config;
use crate::sessions::{Registre, activite_affichee};

pub const PROJET_PRIVE: &str = "Projet privé";
pub const LIMITE_DISCORD: usize = 128;
/// Taille du cache de résolution (`functools.lru_cache(maxsize=128)` en Python).
const TAILLE_CACHE: usize = 128;

#[derive(Clone, Debug, PartialEq)]
pub struct Carte {
    pub details: String,
    pub state: String,
    pub debut: i64,
    pub large_image: &'static str,
    pub large_text: &'static str,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    pub modele: Option<String>,
    pub tokens_session: i64,
    pub tokens_jour: i64,
}

pub fn formater_nombre(n: i64) -> String {
    if n < 1_000 {
        return n.to_string();
    }
    if n < 1_000_000 {
        return format!("{} k", n.div_euclid(1_000));
    }
    format!("{:.1} M", n as f64 / 1_000_000.0).replace('.', ",")
}

/// `s` est-elle non vide, et faite uniquement de chiffres ASCII ?
fn chiffres(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// Équivalent de `^claude-(opus|sonnet|haiku|fable)-(\d+)(?:-(\d{1,2}))?(?:-\d{8})?$`
/// (le `$` de Python accepte un `\n` final unique). Chiffres ASCII seulement.
fn analyser_modele(model_id: &str) -> Option<(&str, &str, Option<&str>)> {
    let coeur = model_id.strip_prefix("claude-")?;
    let coeur = coeur.strip_suffix('\n').unwrap_or(coeur);
    let (famille, reste) = coeur.split_once('-')?;
    if !matches!(famille, "opus" | "sonnet" | "haiku" | "fable") {
        return None;
    }
    let fin_majeur = reste
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(reste.len());
    let (majeur, suite) = reste.split_at(fin_majeur);
    if majeur.is_empty() {
        return None;
    }
    let date = |s: &str| s.len() == 8 && chiffres(s);
    let mineur = |s: &str| (1..=2).contains(&s.len()) && chiffres(s);
    let segments: Vec<&str> = if suite.is_empty() {
        Vec::new()
    } else {
        suite.strip_prefix('-')?.split('-').collect()
    };
    let version_mineure = match segments.as_slice() {
        [] => None,
        [a] if mineur(a) => Some(*a),
        [a] if date(a) => None,
        [a, b] if mineur(a) && date(b) => Some(*a),
        _ => return None,
    };
    Some((famille, majeur, version_mineure))
}

fn capitaliser(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(p) => p
            .to_uppercase()
            .chain(c.flat_map(char::to_lowercase))
            .collect(),
        None => String::new(),
    }
}

pub fn nom_modele(model_id: Option<&str>) -> Option<String> {
    let model_id = model_id.filter(|m| !m.is_empty())?;
    match analyser_modele(model_id) {
        None => Some(
            model_id
                .strip_prefix("claude-")
                .unwrap_or(model_id)
                .to_string(),
        ),
        Some((famille, majeur, mineur)) => {
            let version = match mineur {
                Some(m) => format!("{majeur}.{m}"),
                None => majeur.to_string(),
            };
            Some(format!("{} {version}", capitaliser(famille)))
        }
    }
}

type Cache = Mutex<HashMap<String, Option<PathBuf>>>;

static CACHE_RESOLUTION: LazyLock<Cache> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Résolution mise en cache par dossier : appelée chaque seconde par la boucle
/// carte, un `resolve()` peut bloquer si le volume qui porte le projet est mal
/// monté. La résolution se fait hors verrou. Cache vidé quand il est plein.
fn resoudre_en_cache(
    cache: &Cache,
    cwd: &str,
    resolveur: impl FnOnce(&str) -> Option<PathBuf>,
) -> Option<PathBuf> {
    {
        let c = cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(v) = c.get(cwd) {
            return v.clone();
        }
    }
    let valeur = resolveur(cwd);
    let mut c = cache.lock().unwrap_or_else(|e| e.into_inner());
    if c.len() >= TAILLE_CACHE {
        c.clear();
    }
    c.insert(cwd.to_string(), valeur.clone());
    valeur
}

fn resoudre_cwd(cwd: &str) -> Option<PathBuf> {
    resoudre(Path::new(cwd), false).ok()
}

fn nom_projet_avec(
    cache: &Cache,
    cwd: Option<&str>,
    autorises: &[PathBuf],
    resolveur: impl FnOnce(&str) -> Option<PathBuf>,
) -> String {
    // Comparaison sur chemins complets résolus : un homonyme ou un dossier dont
    // le nom commence pareil n'est jamais autorisé par erreur.
    let Some(cwd) = cwd.filter(|c| !c.is_empty()) else {
        return PROJET_PRIVE.to_string();
    };
    let Some(chemin) = resoudre_en_cache(cache, cwd, resolveur) else {
        return PROJET_PRIVE.to_string();
    };
    for autorise in autorises {
        if chemin == *autorise || chemin.ancestors().skip(1).any(|a| a == autorise) {
            return autorise
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
        }
    }
    PROJET_PRIVE.to_string()
}

pub fn nom_projet(cwd: Option<&str>, autorises: &[PathBuf]) -> String {
    nom_projet_avec(&CACHE_RESOLUTION, cwd, autorises, resoudre_cwd)
}

pub fn tronquer(texte: &str) -> String {
    if texte.chars().count() <= LIMITE_DISCORD {
        return texte.to_string();
    }
    let mut coupe: String = texte.chars().take(LIMITE_DISCORD - 1).collect();
    coupe.push('…');
    coupe
}

/// `a // b` de Python sur des flottants (`float_divmod` de CPython) : pas
/// toujours égal à `floor(a / b)`. Rend `None` là où Python lève (division par
/// zéro, résultat que `int()` refuse).
fn division_entiere_python(a: f64, b: f64) -> Option<f64> {
    if b == 0.0 {
        return None;
    }
    let reste = a % b;
    let mut quotient = (a - reste) / b;
    if reste != 0.0 && (b < 0.0) != (reste < 0.0) {
        quotient -= 1.0;
    }
    let plancher = if quotient != 0.0 {
        let p = quotient.floor();
        if quotient - p > 0.5 { p + 1.0 } else { p }
    } else {
        0.0
    };
    plancher.is_finite().then_some(plancher)
}

/// La carte B (nombre de sessions et cumul) s'affiche-t-elle à cet instant ? `false` là
/// où Python lèverait une exception.
pub fn est_vue_b(maintenant: f64, rotation_s: f64) -> bool {
    division_entiere_python(maintenant, rotation_s).is_some_and(|q| q.rem_euclid(2.0) == 1.0)
}

pub fn construire_carte(
    registre: &Registre,
    config: &Config,
    stats: &Stats,
    maintenant: f64,
    vue_b: bool,
) -> Option<Carte> {
    // Masquage (décision de Victor, 2026-09-18) : une session sans événement
    // depuis `masquer_apres_min` ne compte plus pour la carte, ni affichée ni
    // comptée ni dans le chrono. Calculé sur ce sous-ensemble, jamais sur
    // `registre.session_affichee()` / `debut_plus_ancien()`.
    let masquer_apres_s = config.masquer_apres_min * 60.0;
    let comptent: Vec<_> = registre
        .sessions
        .values()
        .filter(|s| maintenant - s.dernier_evenement < masquer_apres_s)
        .collect();
    // Égalité : première dans l'ordre d'insertion (comme `max` en Python).
    let mut session = *comptent.first()?;
    for s in &comptent {
        if s.dernier_evenement > session.dernier_evenement {
            session = s;
        }
    }
    let activite = activite_affichee(session, maintenant, config.inactif_apres_min * 60.0);
    let details = format!(
        "{activite} · {}",
        nom_projet(session.cwd.as_deref(), &config.projets_autorises)
    );
    let state = if vue_b {
        let n = comptent.len();
        let pluriel = if n > 1 { "s" } else { "" };
        format!(
            "{n} session{pluriel} · {} aujourd'hui",
            formater_nombre(stats.tokens_jour)
        )
    } else {
        let tokens = format!("{} tokens", formater_nombre(stats.tokens_session));
        match nom_modele(stats.modele.as_deref()) {
            Some(modele) => format!("{modele} · {tokens}"),
            None => tokens,
        }
    };
    let debut = comptent
        .iter()
        .map(|s| s.debut)
        .fold(f64::INFINITY, f64::min);
    // Discord refuse toute mise à jour dont `details` ou `state` dépasse 128
    // caractères (déconnexion, puis nouvel essai toutes les 15 s).
    Some(Carte {
        details: tronquer(&details),
        state: tronquer(&state),
        debut: debut as i64,
        large_image: "logo",
        large_text: "Claude Code",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::Evenement;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn ev(nom: &str, sid: &str, t: f64, cwd: Option<&str>, outil: Option<&str>) -> Evenement {
        Evenement {
            nom: nom.into(),
            session_id: sid.into(),
            recu_a: t,
            cwd: cwd.map(String::from),
            tool_name: outil.map(String::from),
            agent_id: None,
            transcript_path: None,
            pid: None,
        }
    }

    fn registre(sessions: &[(&str, f64, Option<&str>)]) -> Registre {
        let mut r = Registre::default();
        for (sid, debut, cwd) in sessions {
            r.appliquer(&ev("SessionStart", sid, *debut, *cwd, None));
        }
        r
    }

    fn stats(modele: Option<&str>, session: i64, jour: i64) -> Stats {
        Stats {
            modele: modele.map(String::from),
            tokens_session: session,
            tokens_jour: jour,
        }
    }

    fn resolu(p: &Path) -> PathBuf {
        resoudre(p, true).unwrap()
    }

    #[test]
    fn test_formater_nombre() {
        let cas = [
            (0, "0"),
            (850, "850"),
            (999, "999"),
            (1_000, "1 k"),
            (445_439, "445 k"),
            (999_999, "999 k"),
            (1_000_000, "1,0 M"),
            (7_721_871, "7,7 M"),
        ];
        for (n, attendu) in cas {
            assert_eq!(formater_nombre(n), attendu, "n = {n}");
        }
    }

    #[test]
    fn test_nom_modele() {
        let cas = [
            (Some("claude-opus-5"), Some("Opus 5")),
            (Some("claude-sonnet-5"), Some("Sonnet 5")),
            (Some("claude-sonnet-4-6"), Some("Sonnet 4.6")),
            (Some("claude-haiku-4-5-20251001"), Some("Haiku 4.5")),
            (Some("claude-fable-5-1"), Some("Fable 5.1")),
            (Some("claude-mystere-9"), Some("mystere-9")),
            (Some("gpt-autre"), Some("gpt-autre")),
            (None, None),
            (Some(""), None),
            (Some("claude-opus-5\n"), Some("Opus 5")),
            (Some("claude-opus-5-20251001"), Some("Opus 5")),
            (Some("claude-opus-5-12345678"), Some("Opus 5")),
            (Some("claude-opus-5-1-2"), Some("opus-5-1-2")),
            (Some("claude-opus-5\n\n"), Some("opus-5\n\n")),
        ];
        for (id, attendu) in cas {
            assert_eq!(nom_modele(id).as_deref(), attendu, "id = {id:?}");
        }
    }

    #[test]
    fn test_nom_projet_autorise_exact() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("MonProjet");
        std::fs::create_dir(&p).unwrap();
        let r = resolu(&p);
        assert_eq!(nom_projet(p.to_str(), &[r]), "MonProjet");
    }

    #[test]
    fn test_nom_projet_sous_dossier_et_worktree() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("MonProjet");
        let wt = p.join(".claude").join("worktrees").join("feature-x");
        std::fs::create_dir_all(&wt).unwrap();
        assert_eq!(nom_projet(wt.to_str(), &[resolu(&p)]), "MonProjet");
    }

    #[test]
    fn test_nom_projet_homonyme_non_autorise() {
        let t = tempfile::tempdir().unwrap();
        let autorise = t.path().join("perso").join("Projet");
        let client = t.path().join("client").join("Projet");
        std::fs::create_dir_all(&autorise).unwrap();
        std::fs::create_dir_all(&client).unwrap();
        assert_eq!(
            nom_projet(client.to_str(), &[resolu(&autorise)]),
            "Projet privé"
        );
    }

    #[test]
    fn test_nom_projet_prefixe_de_nom_non_autorise() {
        let t = tempfile::tempdir().unwrap();
        let autorise = t.path().join("app");
        let voisin = t.path().join("app-client");
        std::fs::create_dir(&autorise).unwrap();
        std::fs::create_dir(&voisin).unwrap();
        assert_eq!(
            nom_projet(voisin.to_str(), &[resolu(&autorise)]),
            "Projet privé"
        );
    }

    #[test]
    fn test_nom_projet_liste_vide_ou_cwd_absent() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(nom_projet(t.path().to_str(), &[]), "Projet privé");
        assert_eq!(nom_projet(None, &[resolu(t.path())]), "Projet privé");
    }

    #[test]
    fn test_est_vue_b_alterne() {
        let r: Vec<bool> = [0.0, 19.9, 20.0, 39.9, 40.0]
            .iter()
            .map(|t| est_vue_b(*t, 20.0))
            .collect();
        assert_eq!(r, [false, false, true, true, false]);
    }

    #[test]
    fn est_vue_b_exceptions_python() {
        // Là où Python lève : division par zéro, NaN, dépassement.
        assert!(!est_vue_b(1.0, 0.0));
        assert!(!est_vue_b(f64::NAN, 2.0));
        assert!(!est_vue_b(2_147_483_648.0, 5e-324));
        // `-1.0 // inf` vaut -1.0 en Python (donc impair), pas -0.0.
        assert!(est_vue_b(-1.0, f64::INFINITY));
    }

    #[test]
    fn test_carte_vue_a() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("MonProjet");
        std::fs::create_dir(&p).unwrap();
        let mut r = registre(&[("s1", 1000.0, p.to_str())]);
        r.appliquer(&ev("PreToolUse", "s1", 1010.0, None, Some("Edit")));
        let cfg = Config {
            projets_autorises: vec![resolu(&p)],
            ..Config::default()
        };
        let carte = construire_carte(
            &r,
            &cfg,
            &stats(Some("claude-opus-5"), 445_439, 8_400_000),
            1011.0,
            false,
        );
        assert_eq!(
            carte,
            Some(Carte {
                details: "Écrit du code · MonProjet".into(),
                state: "Opus 5 · 445 k tokens".into(),
                debut: 1000,
                large_image: "logo",
                large_text: "Claude Code",
            })
        );
    }

    #[test]
    fn test_carte_vue_b_pluriel() {
        let r = registre(&[
            ("s1", 1000.0, None),
            ("s2", 900.0, None),
            ("s3", 950.0, None),
        ]);
        let carte = construire_carte(
            &r,
            &Config::default(),
            &stats(None, 0, 8_400_000),
            1001.0,
            true,
        )
        .unwrap();
        assert_eq!(carte.state, "3 sessions · 8,4 M aujourd'hui");
        assert_eq!(carte.debut, 900);
        assert!(carte.details.ends_with(" · Projet privé"));
    }

    #[test]
    fn test_carte_vue_b_singulier() {
        let r = registre(&[("s1", 1000.0, None)]);
        let carte =
            construire_carte(&r, &Config::default(), &stats(None, 0, 120), 1001.0, true).unwrap();
        assert_eq!(carte.state, "1 session · 120 aujourd'hui");
    }

    #[test]
    fn test_carte_sans_modele() {
        let r = registre(&[("s1", 1000.0, None)]);
        let carte = construire_carte(
            &r,
            &Config::default(),
            &stats(None, 445_439, 0),
            1001.0,
            false,
        )
        .unwrap();
        assert_eq!(carte.state, "445 k tokens");
    }

    #[test]
    fn test_carte_inactive() {
        let r = registre(&[("s1", 1000.0, None)]);
        let cfg = Config {
            inactif_apres_min: 10.0,
            ..Config::default()
        };
        let carte = construire_carte(&r, &cfg, &Stats::default(), 1600.0, false).unwrap();
        assert_eq!(carte.details, "Inactif · Projet privé");
    }

    #[test]
    fn test_pas_de_carte_sans_session() {
        assert_eq!(
            construire_carte(
                &Registre::default(),
                &Config::default(),
                &Stats::default(),
                0.0,
                false
            ),
            None
        );
    }

    #[test]
    fn test_nom_projet_resolution_mise_en_cache() {
        let cache: Cache = Mutex::new(HashMap::new());
        let compteur = AtomicUsize::new(0);
        let p = PathBuf::from("/chemin/MonProjet");
        let autorises = [p.clone()];
        for _ in 0..2 {
            let nom = nom_projet_avec(&cache, Some("/chemin/MonProjet"), &autorises, |c| {
                compteur.fetch_add(1, Ordering::SeqCst);
                Some(PathBuf::from(c))
            });
            assert_eq!(nom, "MonProjet");
        }
        assert_eq!(compteur.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn cache_vide_quand_il_est_plein() {
        let cache: Cache = Mutex::new(HashMap::new());
        for i in 0..TAILLE_CACHE {
            resoudre_en_cache(&cache, &format!("/d{i}"), |c| Some(PathBuf::from(c)));
        }
        assert_eq!(cache.lock().unwrap().len(), TAILLE_CACHE);
        resoudre_en_cache(&cache, "/nouveau", |c| Some(PathBuf::from(c)));
        assert_eq!(cache.lock().unwrap().len(), 1);
    }

    #[test]
    fn test_details_tronque_a_128_caracteres() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("x".repeat(200));
        std::fs::create_dir(&p).unwrap();
        let r = registre(&[("s1", 1000.0, p.to_str())]);
        let cfg = Config {
            projets_autorises: vec![resolu(&p)],
            ..Config::default()
        };
        let carte = construire_carte(&r, &cfg, &Stats::default(), 1001.0, false).unwrap();
        assert_eq!(carte.details.chars().count(), 128);
        assert!(carte.details.ends_with('…'));
    }

    #[test]
    fn test_state_tronque_a_128_caracteres() {
        let r = registre(&[("s1", 1000.0, None)]);
        let s = Stats {
            modele: Some("x".repeat(200)),
            ..Stats::default()
        };
        let carte = construire_carte(&r, &Config::default(), &s, 1001.0, false).unwrap();
        assert_eq!(carte.state.chars().count(), 128);
        assert!(carte.state.ends_with('…'));
    }

    #[test]
    fn tronquer_coupe_en_caracteres_sans_paniquer() {
        let texte = "é".repeat(200);
        let t = tronquer(&texte);
        assert_eq!(t.chars().count(), 128);
        assert!(t.ends_with('…'));
        assert_eq!(tronquer(&"🦀".repeat(128)), "🦀".repeat(128));
    }

    #[test]
    fn test_session_compte_juste_avant_le_masquage() {
        let r = registre(&[("s1", 1000.0, None)]);
        let carte = construire_carte(
            &r,
            &Config::default(),
            &Stats::default(),
            1000.0 + 59.0 * 60.0,
            false,
        );
        assert!(carte.is_some());
    }

    #[test]
    fn test_session_masquee_a_lheure() {
        let r = registre(&[("s1", 1000.0, None)]);
        let carte = construire_carte(
            &r,
            &Config::default(),
            &Stats::default(),
            1000.0 + 60.0 * 60.0,
            false,
        );
        assert!(carte.is_none());
    }

    #[test]
    fn test_deux_sessions_une_masquee_compteur_et_chrono_sur_celle_qui_compte() {
        let r = registre(&[("ancienne", 500.0, None), ("recente", 1000.0, None)]);
        let maintenant = 500.0 + 61.0 * 60.0;
        let carte =
            construire_carte(&r, &Config::default(), &Stats::default(), maintenant, true).unwrap();
        assert!(carte.state.starts_with("1 session "));
        assert_eq!(carte.debut, 1000);
    }

    #[test]
    fn test_session_masquee_recompte_apres_evenement_avec_son_debut_dorigine() {
        let mut r = registre(&[("ancienne", 500.0, None), ("recente", 1000.0, None)]);
        let maintenant = 500.0 + 61.0 * 60.0;
        let carte =
            construire_carte(&r, &Config::default(), &Stats::default(), maintenant, true).unwrap();
        assert_eq!(carte.debut, 1000);

        r.appliquer(&ev(
            "PreToolUse",
            "ancienne",
            maintenant,
            None,
            Some("Edit"),
        ));
        let carte =
            construire_carte(&r, &Config::default(), &Stats::default(), maintenant, true).unwrap();
        assert!(carte.state.starts_with("2 sessions "));
        assert_eq!(carte.debut, 500);
    }

    #[test]
    fn test_masquer_apres_min_inferieur_a_inactif_apres_min_masque_sans_erreur() {
        let r = registre(&[("s1", 1000.0, None)]);
        let cfg = Config {
            masquer_apres_min: 5.0,
            inactif_apres_min: 10.0,
            ..Config::default()
        };
        assert!(
            construire_carte(&r, &cfg, &Stats::default(), 1000.0 + 6.0 * 60.0, false).is_none()
        );
    }
}

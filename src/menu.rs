//! Barre de menus macOS (tray-icon + tao). Voir la spec, sections 8 et 14.2.
//!
//! La logique pure (entrées, icônes) est testée ; la boucle d'interface, elle, est validée à
//! la main.

use std::ffi::OsStr;
use std::path::Path;
use std::sync::Arc;

use crate::daemon::{Daemon, EnMarche, Mode};
use crate::projets::{ConfigEditable, ProjetAffiche};

/// Une ligne du menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entree {
    pub libelle: &'static str,
    pub active: bool,
}

/// Les cinq lignes du menu, dans l'ordre : état, Allumer, Mettre en pause, Arrêter, Quitter.
pub fn entrees(mode: Mode) -> [Entree; 5] {
    [
        Entree {
            libelle: mode.libelle(),
            active: false,
        },
        Entree {
            libelle: "Allumer",
            active: mode != Mode::Allume,
        },
        Entree {
            libelle: "Mettre en pause",
            active: mode == Mode::Allume,
        },
        Entree {
            libelle: "Arrêter",
            active: mode != Mode::Arrete,
        },
        Entree {
            libelle: "Quitter",
            active: true,
        },
    ]
}

pub const COTE_ICONE: u32 = 36;

/// Dessin sur grille 18 × 18 : `#` opaque, `.` transparent. Chaque case fait 2 × 2 pixels.
const GRILLE_ALLUME: &str = "\
..................
...#############..
..###############.
..#######.#######.
..####.##.##.####.
..#####.#.#.#####.
..######...######.
..###.........###.
..######...######.
..#####.#.#.#####.
..####.##.##.####.
..#######.#######.
..###############.
...#############..
....###...........
....##............
....#.............
..................
";

const GRILLE_PAUSE: &str = "\
..................
...#############..
..#.............#.
..#.............#.
..#...##...##...#.
..#...##...##...#.
..#...##...##...#.
..#...##...##...#.
..#...##...##...#.
..#...##...##...#.
..#...##...##...#.
..#.............#.
..#.............#.
...#...#########..
....#.#...........
....##............
....#.............
..................
";

const GRILLE_ARRETE: &str = "\
..................
...#############..
..#.............#.
..#.............#.
..#.............#.
..#.............#.
..#.............#.
..#.............#.
..#.............#.
..#.............#.
..#.............#.
..#.............#.
..#.............#.
...#...#########..
....#.#...........
....##............
....#.............
..................
";

/// Icône RGBA 36×36, noire avec un canal alpha (forme « template » : macOS la teinte),
/// dessinée depuis la grille du mode.
pub fn icone(mode: Mode) -> Vec<u8> {
    let grille = match mode {
        Mode::Allume => GRILLE_ALLUME,
        Mode::Pause => GRILLE_PAUSE,
        Mode::Arrete => GRILLE_ARRETE,
    };
    let lignes: Vec<&[u8]> = grille.lines().map(str::as_bytes).collect();
    let mut rgba = Vec::with_capacity((COTE_ICONE * COTE_ICONE * 4) as usize);
    for y in 0..COTE_ICONE as usize {
        for x in 0..COTE_ICONE as usize {
            let alpha = if lignes[y / 2][x / 2] == b'#' { 255 } else { 0 };
            rgba.extend_from_slice(&[0, 0, 0, alpha]);
        }
    }
    rgba
}

pub const ID_AJOUTER: &str = "projets:ajouter";

/// Action d'un projet du sous-menu « Projets ».
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionMenu {
    Renommer,
    ChangerDossier,
    Retirer,
}

/// Identifiant d'une entrée de projet : `projets:<action>:<index>`.
pub fn id_projet(action: ActionMenu, index: usize) -> String {
    let nom = match action {
        ActionMenu::Renommer => "renommer",
        ActionMenu::ChangerDossier => "changer",
        ActionMenu::Retirer => "retirer",
    };
    format!("projets:{nom}:{index}")
}

/// Inverse de `id_projet` ; `None` pour tout autre identifiant.
pub fn lire_id_projet(id: &str) -> Option<(ActionMenu, usize)> {
    let reste = id.strip_prefix("projets:")?;
    let (nom, index) = reste.split_once(':')?;
    let action = match nom {
        "renommer" => ActionMenu::Renommer,
        "changer" => ActionMenu::ChangerDossier,
        "retirer" => ActionMenu::Retirer,
        _ => return None,
    };
    Some((action, index.parse().ok()?))
}

/// Texte affiché par muda : sous macOS il retire chaque `&` (mnémonique) et lit `&&` comme `&`.
pub fn texte_menu(s: &str) -> String {
    s.replace('&', "&&")
}

/// Ce que le sous-menu « Projets » affiche.
#[derive(Debug, PartialEq, Eq)]
pub enum LignesProjets {
    /// Les projets, déjà triés ; vide : « Aucun projet ».
    Liste(Vec<ProjetAffiche>),
    /// `config.toml` illisible, invalide ou lien symbolique.
    Invalide,
}

pub fn lignes_projets(config: &Path, home: Option<&OsStr>) -> LignesProjets {
    match ConfigEditable::lire(config, home) {
        Ok(editable) => LignesProjets::Liste(editable.affiches(home)),
        Err(_) => LignesProjets::Invalide,
    }
}

#[cfg(feature = "menu")]
const ID_ALLUMER: &str = "allumer";
#[cfg(feature = "menu")]
const ID_PAUSE: &str = "pause";
#[cfg(feature = "menu")]
const ID_ARRETER: &str = "arreter";
#[cfg(feature = "menu")]
const ID_QUITTER: &str = "quitter";

/// Événements utilisateur de la boucle tao.
#[cfg(feature = "menu")]
enum Evt {
    /// Clic sur une entrée du menu (identifiant de l'entrée).
    Clic(String),
    /// Clic sur une action d'un projet ; le chemin est résolu au moment du clic.
    ClicProjet(ActionMenu, String),
    /// Un changement de mode vient de se terminer : redessiner.
    ModeChange,
    /// La config a été rechargée : reconstruire le sous-menu « Projets ».
    ProjetsChanges,
    /// « Quitter », SIGTERM ou SIGINT.
    Quitter,
}

#[cfg(feature = "menu")]
struct Interface {
    tray: tray_icon::TrayIcon,
    lignes: Vec<tray_icon::menu::MenuItem>,
    projets: tray_icon::menu::Submenu,
}

/// Index d'un projet du sous-menu → son chemin tel qu'écrit dans `config.toml`. Partagé entre
/// le gestionnaire de clics et la boucle.
#[cfg(feature = "menu")]
type CheminsProjets = Arc<std::sync::Mutex<Vec<String>>>;

/// Vide le sous-menu « Projets » puis le remplit et met à jour `chemins`. En cas d'échec la
/// liste reste vide : d'anciens chemins ne doivent jamais répondre aux nouveaux identifiants.
#[cfg(feature = "menu")]
fn remplir_projets(
    sous_menu: &tray_icon::menu::Submenu,
    lignes: &LignesProjets,
    chemins_partages: &CheminsProjets,
) -> Result<(), &'static str> {
    use tray_icon::menu::{MenuItem, MenuItemKind, PredefinedMenuItem, Submenu};

    chemins_partages
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();

    for item in sous_menu.items() {
        match item {
            MenuItemKind::MenuItem(i) => sous_menu.remove(&i),
            MenuItemKind::Submenu(i) => sous_menu.remove(&i),
            MenuItemKind::Predefined(i) => sous_menu.remove(&i),
            MenuItemKind::Check(i) => sous_menu.remove(&i),
            MenuItemKind::Icon(i) => sous_menu.remove(&i),
        }
        .map_err(|_| "menu")?;
    }
    let mut chemins = Vec::new();
    match lignes {
        LignesProjets::Liste(projets) if !projets.is_empty() => {
            for (index, projet) in projets.iter().enumerate() {
                let projet_menu = Submenu::new(texte_menu(&projet.nom), true);
                projet_menu
                    .append(&MenuItem::new(
                        texte_menu(&projet.chemin_affiche),
                        false,
                        None,
                    ))
                    .map_err(|_| "menu")?;
                projet_menu
                    .append(&PredefinedMenuItem::separator())
                    .map_err(|_| "menu")?;
                for (action, libelle) in [
                    (ActionMenu::Renommer, "Renommer…"),
                    (ActionMenu::ChangerDossier, "Changer le dossier…"),
                    (ActionMenu::Retirer, "Retirer…"),
                ] {
                    let ligne = MenuItem::with_id(id_projet(action, index), libelle, true, None);
                    projet_menu.append(&ligne).map_err(|_| "menu")?;
                }
                sous_menu.append(&projet_menu).map_err(|_| "menu")?;
                chemins.push(projet.chemin.clone());
            }
        }
        LignesProjets::Liste(_) => sous_menu
            .append(&MenuItem::new("Aucun projet", false, None))
            .map_err(|_| "menu")?,
        LignesProjets::Invalide => sous_menu
            .append(&MenuItem::new("config.toml invalide", false, None))
            .map_err(|_| "menu")?,
    }
    sous_menu
        .append(&PredefinedMenuItem::separator())
        .map_err(|_| "menu")?;
    sous_menu
        .append(&MenuItem::with_id(
            ID_AJOUTER,
            "Ajouter un projet…",
            true,
            None,
        ))
        .map_err(|_| "menu")?;
    *chemins_partages.lock().unwrap_or_else(|e| e.into_inner()) = chemins;
    Ok(())
}

/// Remet à `false` le drapeau d'édition, même si l'édition panique.
#[cfg(feature = "menu")]
struct FinEdition;

#[cfg(feature = "menu")]
impl Drop for FinEdition {
    fn drop(&mut self) {
        EDITION_EN_COURS.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Une seule édition de projet à la fois : un clic pendant un dialogue est ignoré.
#[cfg(feature = "menu")]
static EDITION_EN_COURS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Lance l'édition sur un fil à part : les dialogues bloquent jusqu'à la réponse.
#[cfg(feature = "menu")]
fn lancer_edition(daemon: &Arc<Daemon>, action: crate::edition::Action) {
    use std::sync::atomic::Ordering;

    if EDITION_EN_COURS
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    let daemon = Arc::clone(daemon);
    let lance = std::thread::Builder::new()
        .name("édition".into())
        .spawn(move || {
            let _fin = FinEdition;
            let home = std::env::var_os("HOME");
            let mut lanceur = crate::dialogues::lanceur_reel;
            if crate::edition::editer(
                &action,
                &daemon.chemin_config(),
                home.as_deref(),
                &mut lanceur,
            ) {
                daemon.recharger_config();
            }
        });
    if lance.is_err() {
        log::error!("fil d'édition impossible");
        EDITION_EN_COURS.store(false, Ordering::SeqCst);
    }
}

#[cfg(feature = "menu")]
fn icone_tray(mode: Mode) -> Option<tray_icon::Icon> {
    tray_icon::Icon::from_rgba(icone(mode), COTE_ICONE, COTE_ICONE).ok()
}

#[cfg(feature = "menu")]
fn creer_interface(
    mode: Mode,
    config: &Path,
    chemins: &CheminsProjets,
) -> Result<Interface, &'static str> {
    use tray_icon::TrayIconBuilder;
    use tray_icon::TrayIconEvent;
    use tray_icon::menu::{Menu, MenuItem, PredefinedMenuItem, Submenu};

    let ids = [
        None,
        Some(ID_ALLUMER),
        Some(ID_PAUSE),
        Some(ID_ARRETER),
        Some(ID_QUITTER),
    ];
    let menu = Menu::new();
    let mut lignes = Vec::new();
    let projets = Submenu::new("Projets", true);
    let home = std::env::var_os("HOME");
    remplir_projets(&projets, &lignes_projets(config, home.as_deref()), chemins)?;
    for (entree, id) in entrees(mode).iter().zip(ids) {
        let ligne = match id {
            Some(id) => MenuItem::with_id(id, entree.libelle, entree.active, None),
            None => MenuItem::new(entree.libelle, entree.active, None),
        };
        if id == Some(ID_QUITTER) {
            menu.append(&PredefinedMenuItem::separator())
                .map_err(|_| "menu")?;
            menu.append(&projets).map_err(|_| "menu")?;
            menu.append(&PredefinedMenuItem::separator())
                .map_err(|_| "menu")?;
        }
        menu.append(&ligne).map_err(|_| "menu")?;
        lignes.push(ligne);
    }
    // Sans gestionnaire, tray-icon empile chaque survol dans un canal sans borne, jamais lu.
    TrayIconEvent::set_event_handler(Some(|_| {}));
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon(icone_tray(mode).ok_or("icône")?)
        .with_icon_as_template(true)
        .with_tooltip("Clauderune")
        .build()
        .map_err(|_| "icône de barre de menus")?;
    Ok(Interface {
        tray,
        lignes,
        projets,
    })
}

#[cfg(feature = "menu")]
fn rafraichir(interface: &Interface, mode: Mode) {
    for (ligne, entree) in interface.lignes.iter().zip(entrees(mode)) {
        ligne.set_text(entree.libelle);
        ligne.set_enabled(entree.active);
    }
    if let Some(icone) = icone_tray(mode) {
        let _ = interface.tray.set_icon_with_as_template(Some(icone), true);
    }
}

/// Boucle de la barre de menus, sur le fil principal. Ne rend jamais la main : « Quitter »,
/// SIGTERM et SIGINT arrêtent le daemon puis terminent le processus.
///
/// `changer_mode` peut bloquer plusieurs secondes si Discord est figé : un clic le lance sur
/// un fil à part, qui prévient la boucle par un événement utilisateur une fois fini.
#[cfg(feature = "menu")]
pub fn lancer_menu(daemon: Arc<Daemon>, en_marche: EnMarche) -> ! {
    use crate::edition::Action;
    use std::sync::Mutex;
    use tao::event::{Event, StartCause};
    use tao::event_loop::{ControlFlow, EventLoopBuilder};
    use tray_icon::menu::MenuEvent;

    let mut boucle = EventLoopBuilder::<Evt>::with_user_event().build();
    // Pas d'icône dans le Dock, même hors bundle.
    #[cfg(target_os = "macos")]
    {
        use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
        boucle.set_activation_policy(ActivationPolicy::Accessory);
    }
    let proxy = boucle.create_proxy();

    let chemins: CheminsProjets = Arc::default();
    let chemins_clic = Arc::clone(&chemins);
    let proxy_menu = Mutex::new(proxy.clone());
    // Appelé de façon synchrone sur le fil principal : l'index est résolu en chemin ici, avant
    // qu'une reconstruction du sous-menu en file puisse le décaler.
    MenuEvent::set_event_handler(Some(move |evenement: MenuEvent| {
        let evt = match lire_id_projet(&evenement.id.0) {
            Some((action, index)) => {
                let chemin = chemins_clic
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(index)
                    .cloned();
                match chemin {
                    Some(chemin) => Evt::ClicProjet(action, chemin),
                    None => return,
                }
            }
            None => Evt::Clic(evenement.id.0),
        };
        if let Ok(proxy) = proxy_menu.lock() {
            let _ = proxy.send_event(evt);
        }
    }));
    let proxy_signal = proxy.clone();
    if ctrlc::set_handler(move || {
        let _ = proxy_signal.send_event(Evt::Quitter);
    })
    .is_err()
    {
        log::warn!("gestionnaire de signaux non installé");
    }

    let mut en_marche = Some(en_marche);
    let mut interface: Option<Interface> = None;
    boucle.run(move |evenement, _, flux| {
        *flux = ControlFlow::Wait;
        match evenement {
            // Sur macOS, l'icône se crée dans `Init`, comme le recommande tray-icon.
            Event::NewEvents(StartCause::Init) => {
                // Rappel installé avant l'interface : un `ProjetsChanges` reçu trop tôt est
                // ignoré. Il part du fil de contrôle ou d'édition et n'envoie qu'un événement.
                let proxy_projets = Mutex::new(proxy.clone());
                daemon.sur_config_changee(Box::new(move || {
                    if let Ok(proxy) = proxy_projets.lock() {
                        let _ = proxy.send_event(Evt::ProjetsChanges);
                    }
                }));
                match creer_interface(daemon.mode(), &daemon.chemin_config(), &chemins) {
                    Ok(i) => interface = Some(i),
                    Err(genre) => {
                        log::error!("barre de menus impossible ({genre})");
                        if let Some(m) = en_marche.take() {
                            m.arreter();
                        }
                        std::process::exit(1);
                    }
                }
            }
            Event::UserEvent(Evt::ClicProjet(action, chemin)) => lancer_edition(
                &daemon,
                match action {
                    ActionMenu::Renommer => Action::Renommer(chemin),
                    ActionMenu::ChangerDossier => Action::ChangerDossier(chemin),
                    ActionMenu::Retirer => Action::Retirer(chemin),
                },
            ),
            Event::UserEvent(Evt::Clic(id)) => {
                if id == ID_AJOUTER {
                    lancer_edition(&daemon, Action::Ajouter);
                }
                let mode = match id.as_str() {
                    ID_ALLUMER => Some(Mode::Allume),
                    ID_PAUSE => Some(Mode::Pause),
                    ID_ARRETER => Some(Mode::Arrete),
                    ID_QUITTER => {
                        let _ = proxy.send_event(Evt::Quitter);
                        None
                    }
                    _ => None,
                };
                if let Some(mode) = mode {
                    let (daemon, proxy) = (Arc::clone(&daemon), proxy.clone());
                    std::thread::spawn(move || {
                        daemon.changer_mode(mode);
                        let _ = proxy.send_event(Evt::ModeChange);
                    });
                }
            }
            Event::UserEvent(Evt::ModeChange) => {
                if let Some(i) = &interface {
                    rafraichir(i, daemon.mode());
                }
            }
            Event::UserEvent(Evt::ProjetsChanges) => {
                if let Some(i) = &interface {
                    let home = std::env::var_os("HOME");
                    let lignes = lignes_projets(&daemon.chemin_config(), home.as_deref());
                    match remplir_projets(&i.projets, &lignes, &chemins) {
                        Ok(()) => {}
                        Err(genre) => log::error!("sous-menu Projets impossible ({genre})"),
                    }
                }
            }
            Event::UserEvent(Evt::Quitter) => {
                if let Some(m) = en_marche.take() {
                    m.arreter();
                }
                std::process::exit(0);
            }
            _ => {}
        }
    })
}

#[cfg(all(not(feature = "menu"), unix))]
static SIGNAL_RECU: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(all(not(feature = "menu"), unix))]
extern "C" fn sur_signal(_: libc::c_int) {
    SIGNAL_RECU.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// Sans la feature `menu` : pas d'interface, le daemon tourne jusqu'à SIGTERM ou SIGINT.
/// Utile au débogage.
#[cfg(not(feature = "menu"))]
pub fn lancer_menu(_daemon: Arc<Daemon>, en_marche: EnMarche) -> ! {
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    for signal in [libc::SIGTERM, libc::SIGINT] {
        // SAFETY : le gestionnaire ne fait qu'une écriture atomique, sûre en contexte de signal.
        unsafe { libc::signal(signal, sur_signal as *const () as libc::sighandler_t) };
    }
    while !SIGNAL_RECU.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(100));
    }
    en_marche.arreter();
    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entrees_selon_le_mode() {
        let e = entrees(Mode::Pause);
        assert_eq!(e[0].libelle, "En pause");
        assert!(!e[0].active);
        assert!(e[1].active);
        assert!(!e[2].active);
        assert!(e[3].active);
        assert!(e[4].active);
    }

    #[test]
    fn entrees_allume_et_arrete() {
        let e = entrees(Mode::Allume);
        assert_eq!(e[0].libelle, "Allumé");
        assert!(!e[1].active && e[2].active && e[3].active);
        let e = entrees(Mode::Arrete);
        assert_eq!(e[0].libelle, "Arrêté");
        assert!(e[1].active && !e[2].active && !e[3].active);
        assert_eq!(
            e.iter().map(|x| x.libelle).collect::<Vec<_>>(),
            ["Arrêté", "Allumer", "Mettre en pause", "Arrêter", "Quitter"]
        );
    }

    #[test]
    fn icones_distinctes_et_de_bonne_taille() {
        for m in [Mode::Allume, Mode::Pause, Mode::Arrete] {
            assert_eq!(icone(m).len(), 36 * 36 * 4);
        }
        assert_ne!(icone(Mode::Allume), icone(Mode::Pause));
        assert_ne!(icone(Mode::Pause), icone(Mode::Arrete));
    }

    #[test]
    fn icones_noires_avec_alpha_et_non_vides() {
        for m in [Mode::Allume, Mode::Pause, Mode::Arrete] {
            let rgba = icone(m);
            assert!(rgba.chunks(4).all(|p| p[..3] == [0, 0, 0]));
            assert!(rgba.chunks(4).any(|p| p[3] == 255));
            assert!(rgba.chunks(4).any(|p| p[3] == 0));
        }
        // Corps plein pour la forme allumée ; centre vide pour la boîte vide et pour l'espace entre barres.
        let alpha = |m: Mode, x: usize, y: usize| icone(m)[(y * 36 + x) * 4 + 3];
        assert_eq!(alpha(Mode::Allume, 6, 18), 255);
        assert_eq!(alpha(Mode::Arrete, 18, 18), 0);
        assert_eq!(alpha(Mode::Pause, 18, 18), 0);
    }

    const MODES: [Mode; 3] = [Mode::Allume, Mode::Pause, Mode::Arrete];

    fn grille(mode: Mode) -> &'static str {
        match mode {
            Mode::Allume => GRILLE_ALLUME,
            Mode::Pause => GRILLE_PAUSE,
            Mode::Arrete => GRILLE_ARRETE,
        }
    }

    #[test]
    fn grilles_de_18_sur_18() {
        for m in MODES {
            let lignes: Vec<&str> = grille(m).lines().collect();
            assert_eq!(lignes.len(), 18);
            assert!(lignes.iter().all(|l| l.chars().count() == 18));
            assert!(
                lignes
                    .iter()
                    .all(|l| l.chars().all(|c| c == '#' || c == '.'))
            );
        }
    }

    #[test]
    fn icone_suit_la_grille() {
        for m in MODES {
            let rgba = icone(m);
            for (y, ligne) in grille(m).lines().enumerate() {
                for (x, c) in ligne.chars().enumerate() {
                    let attendu = if c == '#' { 255 } else { 0 };
                    assert_eq!(
                        rgba[((2 * y) * 36 + 2 * x) * 4 + 3],
                        attendu,
                        "{m:?} ({x}, {y})"
                    );
                }
            }
        }
    }

    #[test]
    fn id_projet_aller_retour() {
        for action in [
            ActionMenu::Renommer,
            ActionMenu::ChangerDossier,
            ActionMenu::Retirer,
        ] {
            for index in [0, 41] {
                assert_eq!(
                    lire_id_projet(&id_projet(action, index)),
                    Some((action, index))
                );
            }
        }
        assert_eq!(id_projet(ActionMenu::Renommer, 3), "projets:renommer:3");
        assert_eq!(
            id_projet(ActionMenu::ChangerDossier, 3),
            "projets:changer:3"
        );
        assert_eq!(id_projet(ActionMenu::Retirer, 3), "projets:retirer:3");
    }

    #[test]
    fn lire_id_projet_refuse_les_autres_ids() {
        for id in [
            ID_AJOUTER,
            "projets:renommer:",
            "projets:renommer:x",
            "projets:inconnu:1",
            "allumer",
        ] {
            assert_eq!(lire_id_projet(id), None, "{id}");
        }
    }

    #[test]
    fn lignes_projets_triees() {
        let dossier = tempfile::tempdir().unwrap();
        let config = dossier.path().join("config.toml");
        std::fs::write(
            &config,
            "[projets]\nautorises = [\"/tmp/zebre\", \"/tmp/abeille\"]\n",
        )
        .unwrap();
        let LignesProjets::Liste(liste) = lignes_projets(&config, None) else {
            panic!("liste attendue");
        };
        let noms: Vec<&str> = liste.iter().map(|p| p.nom.as_str()).collect();
        assert_eq!(noms, ["abeille", "zebre"]);
        assert_eq!(liste[0].chemin, "/tmp/abeille");
    }

    #[test]
    fn chemin_affiche_sous_le_dossier_personnel() {
        let home = tempfile::tempdir().unwrap();
        let p = home.path().join("p");
        std::fs::create_dir(&p).unwrap();
        let config = home.path().join("config.toml");
        std::fs::write(
            &config,
            format!("[projets]\nautorises = [\"{}\"]\n", p.display()),
        )
        .unwrap();
        let LignesProjets::Liste(liste) = lignes_projets(&config, Some(home.path().as_os_str()))
        else {
            panic!("liste attendue");
        };
        assert_eq!(liste[0].chemin_affiche, "~/p");
        assert_eq!(liste[0].chemin, p.display().to_string());
    }

    #[test]
    fn lignes_projets_config_invalide() {
        let dossier = tempfile::tempdir().unwrap();
        let config = dossier.path().join("config.toml");
        std::fs::write(&config, "ceci n'est pas = [du toml").unwrap();
        assert_eq!(lignes_projets(&config, None), LignesProjets::Invalide);
    }

    #[test]
    fn texte_menu_double_les_esperluettes() {
        assert_eq!(texte_menu("R&D"), "R&&D");
        assert_eq!(texte_menu("a"), "a");
    }
}

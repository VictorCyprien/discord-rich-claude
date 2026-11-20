//! Barre de menus macOS (tray-icon + tao). Voir la spec, sections 8 et 14.2.
//!
//! La logique pure (entrées, icônes) est testée ; la boucle d'interface, elle, est validée à
//! la main.

use std::sync::Arc;

use crate::daemon::{Daemon, EnMarche, Mode};

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

/// Couverture (0.0 à 1.0) du pixel `(x, y)` par une forme : bord adouci sur un pixel.
fn couverture_disque(x: f64, y: f64, rayon: f64) -> f64 {
    let centre = f64::from(COTE_ICONE) / 2.0;
    let distance = ((x + 0.5 - centre).powi(2) + (y + 0.5 - centre).powi(2)).sqrt();
    (rayon + 0.5 - distance).clamp(0.0, 1.0)
}

/// Icône RGBA 36×36, noire avec un canal alpha (forme « template » : macOS la teinte).
/// Disque plein pour `Allume`, deux barres pour `Pause`, anneau pour `Arrete`.
pub fn icone(mode: Mode) -> Vec<u8> {
    let mut rgba = Vec::with_capacity((COTE_ICONE * COTE_ICONE * 4) as usize);
    for y in 0..COTE_ICONE {
        for x in 0..COTE_ICONE {
            let (fx, fy) = (f64::from(x), f64::from(y));
            let couverture = match mode {
                Mode::Allume => couverture_disque(fx, fy, 14.0),
                Mode::Pause => {
                    let barre = (9..16).contains(&x) || (20..27).contains(&x);
                    if barre && (8..28).contains(&y) {
                        1.0
                    } else {
                        0.0
                    }
                }
                Mode::Arrete => {
                    (couverture_disque(fx, fy, 14.0) - couverture_disque(fx, fy, 10.0)).max(0.0)
                }
            };
            rgba.extend_from_slice(&[0, 0, 0, (couverture * 255.0).round() as u8]);
        }
    }
    rgba
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
    /// Un changement de mode vient de se terminer : redessiner.
    ModeChange,
    /// « Quitter », SIGTERM ou SIGINT.
    Quitter,
}

#[cfg(feature = "menu")]
struct Interface {
    tray: tray_icon::TrayIcon,
    lignes: Vec<tray_icon::menu::MenuItem>,
}

#[cfg(feature = "menu")]
fn icone_tray(mode: Mode) -> Option<tray_icon::Icon> {
    tray_icon::Icon::from_rgba(icone(mode), COTE_ICONE, COTE_ICONE).ok()
}

#[cfg(feature = "menu")]
fn creer_interface(mode: Mode) -> Result<Interface, &'static str> {
    use tray_icon::TrayIconBuilder;
    use tray_icon::TrayIconEvent;
    use tray_icon::menu::{Menu, MenuItem, PredefinedMenuItem};

    let ids = [
        None,
        Some(ID_ALLUMER),
        Some(ID_PAUSE),
        Some(ID_ARRETER),
        Some(ID_QUITTER),
    ];
    let menu = Menu::new();
    let mut lignes = Vec::new();
    for (entree, id) in entrees(mode).iter().zip(ids) {
        let ligne = match id {
            Some(id) => MenuItem::with_id(id, entree.libelle, entree.active, None),
            None => MenuItem::new(entree.libelle, entree.active, None),
        };
        if id == Some(ID_QUITTER) {
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
        .with_tooltip("Discord Rich Claude")
        .build()
        .map_err(|_| "icône de barre de menus")?;
    Ok(Interface { tray, lignes })
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

    let proxy_menu = Mutex::new(proxy.clone());
    MenuEvent::set_event_handler(Some(move |evenement: MenuEvent| {
        if let Ok(proxy) = proxy_menu.lock() {
            let _ = proxy.send_event(Evt::Clic(evenement.id.0));
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
            Event::NewEvents(StartCause::Init) => match creer_interface(daemon.mode()) {
                Ok(i) => interface = Some(i),
                Err(genre) => {
                    log::error!("barre de menus impossible ({genre})");
                    if let Some(m) = en_marche.take() {
                        m.arreter();
                    }
                    std::process::exit(1);
                }
            },
            Event::UserEvent(Evt::Clic(id)) => {
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
        // Le centre : plein pour le disque, vide pour l'anneau et pour l'espace entre barres.
        let alpha = |m: Mode, x: usize, y: usize| icone(m)[(y * 36 + x) * 4 + 3];
        assert_eq!(alpha(Mode::Allume, 18, 18), 255);
        assert_eq!(alpha(Mode::Arrete, 18, 18), 0);
        assert_eq!(alpha(Mode::Pause, 18, 18), 0);
    }
}

//! Bundle `.app` minimal, signé ad hoc, qui porte le binaire du daemon.
//! Voir la spec, sections 8 et 14.4.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::installation::{
    ENTETE_PLIST, Executeur, ecrire_atomique, ecrire_fichier_atomique, plist_chaine,
};

pub const NOM_BUNDLE: &str = "Discord Rich Claude.app";
const NOM_BINAIRE: &str = "discord-rich-claude";

/// `Info.plist` du bundle : clés triées, au format de `plistlib.dumps`.
pub fn info_plist(version: &str) -> Vec<u8> {
    let mut xml = String::from(ENTETE_PLIST);
    xml += &plist_chaine("CFBundleExecutable", NOM_BINAIRE);
    xml += &plist_chaine("CFBundleIdentifier", "local.discord-rich-claude");
    xml += &plist_chaine("CFBundleName", "Discord Rich Claude");
    xml += &plist_chaine("CFBundlePackageType", "APPL");
    xml += &plist_chaine("CFBundleShortVersionString", version);
    xml += &plist_chaine("CFBundleVersion", version);
    xml += "\t<key>LSUIElement</key>\n\t<true/>\n";
    xml += "</dict>\n</plist>\n";
    xml.into_bytes()
}

pub fn binaire_du_bundle(applications: &Path) -> PathBuf {
    applications
        .join(NOM_BUNDLE)
        .join("Contents")
        .join("MacOS")
        .join(NOM_BINAIRE)
}

fn erreur_io(action: &str, e: &io::Error) -> String {
    format!("{action} impossible ({:?})", e.kind())
}

/// Copie `source` vers `cible` via un temporaire frère en 0o755, puis `rename`.
fn copier_binaire(source: &Path, cible: &Path) -> io::Result<()> {
    let mut entree = fs::File::open(source)?;
    let nom = cible
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    // Le temporaire est créé par `ecrire_atomique` ; on y recopie le contenu en flux.
    let mut contenu = Vec::new();
    io::Read::read_to_end(&mut entree, &mut contenu)?;
    ecrire_atomique(cible, &contenu, 0o755, &format!(".{nom}."), |de, vers| {
        fs::rename(de, vers)
    })
}

/// Même fichier une fois les liens résolus (la cible peut ne pas exister encore).
fn est_la_cible(source: &Path, cible: &Path) -> bool {
    match (fs::canonicalize(source), fs::canonicalize(cible)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

pub fn installer_bundle(
    applications: &Path,
    binaire_source: &Path,
    executer: Executeur,
) -> Result<PathBuf, String> {
    let app = applications.join(NOM_BUNDLE);
    let contenu = app.join("Contents");
    fs::create_dir_all(contenu.join("MacOS"))
        .map_err(|e| erreur_io("création de Contents/MacOS", &e))?;
    fs::create_dir_all(contenu.join("Resources"))
        .map_err(|e| erreur_io("création de Contents/Resources", &e))?;
    ecrire_fichier_atomique(
        &contenu.join("Info.plist"),
        &info_plist(env!("CARGO_PKG_VERSION")),
        0o644,
    )
    .map_err(|e| erreur_io("écriture d'Info.plist", &e))?;

    let cible = binaire_du_bundle(applications);
    if !est_la_cible(binaire_source, &cible) {
        copier_binaire(binaire_source, &cible).map_err(|e| erreur_io("copie du binaire", &e))?;
    }

    let chemin_app = app.to_string_lossy().into_owned();
    let sortie = executer(&["codesign", "--force", "--sign", "-", &chemin_app])
        .map_err(|e| erreur_io("lancement de codesign", &e))?;
    if sortie.code != 0 {
        return Err(format!(
            "codesign a échoué (code {}) : {}",
            sortie.code,
            String::from_utf8_lossy(&sortie.stderr).trim()
        ));
    }
    Ok(cible)
}

pub fn desinstaller_bundle(applications: &Path) -> io::Result<()> {
    let app = applications.join(NOM_BUNDLE);
    // Garde contre un effacement hors cible : bon nom, et pas un lien.
    if app.file_name().and_then(|n| n.to_str()) != Some(NOM_BUNDLE) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "chemin de bundle inattendu",
        ));
    }
    match fs::symlink_metadata(&app) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
        Ok(m) if m.file_type().is_symlink() => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "le bundle est un lien, refus de le suivre",
            ));
        }
        Ok(_) => {}
    }
    match fs::remove_dir_all(&app) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::installation::Sortie;
    use std::os::unix::fs::PermissionsExt;

    fn faux_executeur(
        appels: &mut Vec<Vec<String>>,
        code: i32,
    ) -> impl FnMut(&[&str]) -> io::Result<Sortie> + '_ {
        move |cmd| {
            appels.push(cmd.iter().map(|s| s.to_string()).collect());
            Ok(Sortie {
                code,
                stderr: b"signature refusee\n".to_vec(),
            })
        }
    }

    #[test]
    fn installer_bundle_copie_signe_et_pointe_dans_le_bundle() {
        let d = tempfile::tempdir().unwrap();
        let applications = d.path().join("Applications");
        let source = d.path().join("source");
        fs::write(&source, b"binaire factice").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();
        let mut appels = vec![];
        let rendu =
            installer_bundle(&applications, &source, &mut faux_executeur(&mut appels, 0)).unwrap();
        assert_eq!(rendu, binaire_du_bundle(&applications));
        assert_eq!(fs::read(&rendu).unwrap(), b"binaire factice");
        assert_eq!(
            fs::metadata(&rendu).unwrap().permissions().mode() & 0o7777,
            0o755
        );
        let app = applications.join(NOM_BUNDLE);
        assert!(app.join("Contents/Resources").is_dir());
        let info = fs::read_to_string(app.join("Contents/Info.plist")).unwrap();
        assert!(info.contains("LSUIElement"));
        assert_eq!(
            appels,
            vec![vec![
                "codesign".to_string(),
                "--force".into(),
                "--sign".into(),
                "-".into(),
                app.to_string_lossy().into_owned()
            ]]
        );
        // aucun temporaire ne traîne
        let noms: Vec<_> = fs::read_dir(app.join("Contents/MacOS"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(noms.len(), 1);
    }

    #[test]
    fn installer_bundle_depuis_le_bundle_lui_meme_ne_casse_rien() {
        let d = tempfile::tempdir().unwrap();
        let applications = d.path().join("Applications");
        let cible = binaire_du_bundle(&applications);
        fs::create_dir_all(cible.parent().unwrap()).unwrap();
        fs::write(&cible, b"deja la").unwrap();
        let mut appels = vec![];
        let rendu =
            installer_bundle(&applications, &cible, &mut faux_executeur(&mut appels, 0)).unwrap();
        assert_eq!(rendu, cible);
        assert_eq!(fs::read(&cible).unwrap(), b"deja la");
        assert_eq!(appels.len(), 1);
    }

    #[test]
    fn installer_bundle_echec_de_codesign_rend_le_stderr() {
        let d = tempfile::tempdir().unwrap();
        let source = d.path().join("source");
        fs::write(&source, b"x").unwrap();
        let mut appels = vec![];
        let e = installer_bundle(
            &d.path().join("A"),
            &source,
            &mut faux_executeur(&mut appels, 1),
        )
        .unwrap_err();
        assert!(e.contains("signature refusee"));
    }

    #[test]
    fn info_plist_au_format_plistlib() {
        let t = String::from_utf8(info_plist("1.2.3")).unwrap();
        assert!(t.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist"));
        for attendu in [
            "<key>CFBundleExecutable</key>\n\t<string>discord-rich-claude</string>",
            "<key>CFBundleIdentifier</key>\n\t<string>local.discord-rich-claude</string>",
            "<key>CFBundleName</key>\n\t<string>Discord Rich Claude</string>",
            "<key>CFBundlePackageType</key>\n\t<string>APPL</string>",
            "<key>CFBundleShortVersionString</key>\n\t<string>1.2.3</string>",
            "<key>CFBundleVersion</key>\n\t<string>1.2.3</string>",
            "<key>LSUIElement</key>\n\t<true/>\n</dict>\n</plist>\n",
        ] {
            assert!(t.contains(attendu), "manque : {attendu}");
        }
    }

    #[test]
    fn desinstaller_bundle_absent_ne_leve_pas() {
        let d = tempfile::tempdir().unwrap();
        desinstaller_bundle(&d.path().join("Applications")).unwrap();
    }

    #[test]
    fn desinstaller_bundle_supprime_le_app() {
        let d = tempfile::tempdir().unwrap();
        let app = d.path().join(NOM_BUNDLE);
        fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        fs::write(app.join("Contents/MacOS/x"), b"x").unwrap();
        desinstaller_bundle(d.path()).unwrap();
        assert!(!app.exists());
    }

    #[test]
    fn desinstaller_bundle_refuse_un_lien() {
        let d = tempfile::tempdir().unwrap();
        let ailleurs = d.path().join("ailleurs");
        fs::create_dir(&ailleurs).unwrap();
        fs::write(ailleurs.join("precieux"), b"x").unwrap();
        let applications = d.path().join("Applications");
        fs::create_dir(&applications).unwrap();
        std::os::unix::fs::symlink(&ailleurs, applications.join(NOM_BUNDLE)).unwrap();
        assert!(desinstaller_bundle(&applications).is_err());
        assert!(ailleurs.join("precieux").exists());
    }

    #[test]
    fn copier_binaire_ne_laisse_pas_de_temporaire_si_la_source_manque() {
        let d = tempfile::tempdir().unwrap();
        assert!(copier_binaire(&d.path().join("absent"), &d.path().join("cible")).is_err());
        assert_eq!(fs::read_dir(d.path()).unwrap().count(), 0);
    }
}

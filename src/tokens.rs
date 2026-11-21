//! Tokens et modèle lus dans les transcripts JSONL de Claude Code. Voir la spec, section 6.
//!
//! Les transcripts répètent une même réponse avec des compteurs différents (écriture
//! en flux) : on garde une entrée par (message.id, requestId), avec le maximum de
//! chaque champ. Garder la première occurrence sous-compte.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use chrono::{Duration, Local, NaiveDate, NaiveDateTime, Offset, TimeZone};
use serde_json::Value;

// Les lectures du cache sont exclues : elles grossissent avec la longueur du
// contexte, pas avec le travail fait.
pub const CHAMPS_COMPTES: [&str; 3] = [
    "input_tokens",
    "cache_creation_input_tokens",
    "output_tokens",
];

pub type Cle = (String, String);

/// `fold=0` de Python : l'instant le plus tôt, quel que soit l'ordre dans lequel
/// chrono range les deux candidats d'une heure ambiguë (sur `Local`, il est inversé).
fn premier_instant<Tz: TimeZone>(r: chrono::LocalResult<chrono::DateTime<Tz>>) -> Option<i64> {
    match r {
        chrono::LocalResult::Single(d) => Some(d.timestamp()),
        chrono::LocalResult::Ambiguous(a, b) => Some(a.timestamp().min(b.timestamp())),
        chrono::LocalResult::None => None,
    }
}

/// Valeur de `minuit_local` pour un fuseau donné, ou le nom de l'exception Python
/// que `datetime.fromtimestamp` lèverait.
///
/// Minuit est pris dans le fuseau en vigueur à minuit lui-même, pas dans celui de
/// `maintenant` (les deux diffèrent un jour de changement d'heure).
pub fn minuit_selon<Tz: TimeZone>(fuseau: &Tz, maintenant: f64) -> Result<f64, &'static str> {
    if maintenant.is_nan() {
        return Err("ValueError");
    }
    // 2^63 : au-delà, `time_t` déborde.
    if !maintenant.is_finite() || maintenant.abs() >= 9_223_372_036_854_775_808.0 {
        return Err("OverflowError");
    }
    let secondes = maintenant.floor() as i64;
    let maintenant_local = fuseau
        .timestamp_opt(secondes, 0)
        .single()
        .ok_or("ValueError")?;
    let jour = maintenant_local.date_naive();
    if !(1..=9999).contains(&chrono::Datelike::year(&jour)) {
        return Err("ValueError");
    }
    let minuit = jour.and_hms_opt(0, 0, 0).ok_or("ValueError")?;
    let instant = match premier_instant(fuseau.from_local_datetime(&minuit)) {
        Some(t) => t,
        None => {
            // Minuit n'existe pas (saut d'heure) : décalage d'avant la transition.
            let avant = fuseau.offset_from_utc_datetime(&(minuit - Duration::days(1)));
            minuit.and_utc().timestamp() - i64::from(avant.fix().local_minus_utc())
        }
    };
    Ok(instant as f64)
}

/// Comme `minuit_local`, mais rend le nom de l'exception Python là où elle lève.
pub fn minuit_local_strict(maintenant: f64) -> Result<f64, &'static str> {
    minuit_selon(&Local, maintenant)
}

pub fn minuit_local(maintenant: f64) -> f64 {
    minuit_local_strict(maintenant).unwrap_or(0.0)
}

fn finit_par_jsonl(nom: &std::ffi::OsStr) -> bool {
    nom.as_encoded_bytes().ends_with(b".jsonl")
}

pub fn fichiers_de_session(transcript: &Path) -> Vec<PathBuf> {
    let sous_agents = transcript.with_extension("").join("subagents");
    let mut trouves: Vec<PathBuf> = fs::read_dir(&sous_agents)
        .map(|entrees| {
            entrees
                .flatten()
                .filter(|e| finit_par_jsonl(&e.file_name()))
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default();
    trouves.sort();
    let mut tous = vec![transcript.to_path_buf()];
    tous.extend(trouves);
    tous
}

/// Parcours récursif sans suivre les dossiers liés. Garde les entrées dont le nom
/// finit par `.jsonl` (un fichier lié qui pointe vers un fichier reste gardé).
fn parcourir(dossier: &Path, trouves: &mut Vec<PathBuf>) {
    let Ok(entrees) = fs::read_dir(dossier) else {
        return;
    };
    for entree in entrees.flatten() {
        let chemin = entree.path();
        if finit_par_jsonl(&entree.file_name()) {
            trouves.push(chemin.clone());
        }
        if entree.file_type().is_ok_and(|t| t.is_dir()) {
            parcourir(&chemin, trouves);
        }
    }
}

// --- horodatage -----------------------------------------------------------

fn chiffres(s: &[u8]) -> Option<u32> {
    if s.is_empty() || !s.iter().all(u8::is_ascii_digit) {
        return None;
    }
    s.iter().try_fold(0u32, |acc, c| {
        acc.checked_mul(10)?.checked_add(u32::from(c - b'0'))
    })
}

fn seconds_naives(naif: &NaiveDateTime, microsecondes: i64, decalage_s: Option<i64>) -> f64 {
    match decalage_s {
        Some(d) => {
            let total = (naif.and_utc().timestamp() - d) * 1_000_000 + microsecondes;
            total as f64 / 1e6
        }
        None => {
            let s = match premier_instant(Local.from_local_datetime(naif)) {
                Some(t) => t,
                None => {
                    let avant = Local.offset_from_utc_datetime(&(*naif - Duration::days(1)));
                    naif.and_utc().timestamp() - i64::from(avant.fix().local_minus_utc())
                }
            };
            s as f64 + microsecondes as f64 / 1e6
        }
    }
}

/// Formats acceptés (écart connu n° 3) : RFC 3339, `AAAA-MM-JJTHH:MM:SS[.f]` et
/// `AAAA-MM-JJ HH:MM:SS[.f]` sans fuseau (heure locale), `AAAA-MM-JJ`.
pub fn horodatage(brut: &Value) -> Option<f64> {
    let s = brut.as_str()?;
    if !s.is_ascii() || s.len() < 10 {
        return None;
    }
    let o = s.as_bytes();
    if o[4] != b'-' || o[7] != b'-' {
        return None;
    }
    let date = NaiveDate::from_ymd_opt(
        i32::try_from(chiffres(&o[0..4])?).ok()?,
        chiffres(&o[5..7])?,
        chiffres(&o[8..10])?,
    )?;
    if o.len() == 10 {
        return Some(seconds_naives(&date.and_hms_opt(0, 0, 0)?, 0, None));
    }
    if (o[10] != b'T' && o[10] != b' ') || o.len() < 19 || o[13] != b':' || o[16] != b':' {
        return None;
    }
    let heure = chiffres(&o[11..13])?;
    let minute = chiffres(&o[14..16])?;
    let seconde = chiffres(&o[17..19])?;
    let naif = date.and_hms_opt(heure, minute, seconde)?;
    let mut reste = &o[19..];
    let mut micros = 0i64;
    if reste.first() == Some(&b'.') {
        let n = reste[1..].iter().take_while(|c| c.is_ascii_digit()).count();
        if n == 0 {
            return None;
        }
        let mut fraction = reste[1..=n].to_vec();
        fraction.truncate(6);
        while fraction.len() < 6 {
            fraction.push(b'0');
        }
        micros = i64::from(chiffres(&fraction)?);
        reste = &reste[n + 1..];
    }
    let decalage = match reste {
        [] => None,
        [b'Z'] => Some(0),
        [signe @ (b'+' | b'-'), h1, h2, b':', m1, m2] => {
            let h = i64::from(chiffres(&[*h1, *h2])?);
            let m = i64::from(chiffres(&[*m1, *m2])?);
            if h > 23 || m > 59 {
                return None;
            }
            let total = h * 3600 + m * 60;
            Some(if *signe == b'-' { -total } else { total })
        }
        _ => return None,
    };
    Some(seconds_naives(&naif, micros, decalage))
}

// --- extraction -----------------------------------------------------------

/// Vérité Python : `""`, `0`, `0.0`, `null`, `false`, `[]` et `{}` sont faux.
fn est_faux(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Bool(b) => !b,
        Value::Number(n) => n.as_f64() == Some(0.0) || n.as_i64() == Some(0),
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
    }
}

/// `str(x)` de Python pour texte, entier et booléen. Autre forme : écart connu n° 2.
fn en_texte(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Bool(b) => Some(if *b { "True" } else { "False" }.to_string()),
        Value::Number(n) if n.is_i64() || n.is_u64() => Some(n.to_string()),
        _ => None,
    }
}

fn est_blanc_python(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

fn entier_depuis_texte(brut: &str) -> Option<i64> {
    let texte = brut.trim_matches(est_blanc_python);
    let (negatif, corps) = match texte.as_bytes().first()? {
        b'+' => (false, &texte[1..]),
        b'-' => (true, &texte[1..]),
        _ => (false, texte),
    };
    let o = corps.as_bytes();
    if o.is_empty()
        || o[0] == b'_'
        || o[o.len() - 1] == b'_'
        || !o.iter().all(|c| c.is_ascii_digit() || *c == b'_')
        || corps.contains("__")
    {
        return None;
    }
    let nu: String = corps.chars().filter(|c| *c != '_').collect();
    let signe = if negatif { "-" } else { "" };
    format!("{signe}{nu}").parse().ok()
}

/// `int(x or 0)` de Python. `None` : conversion impossible (`TypeError`, `ValueError`)
/// ou hors `i64` (écart connu n° 2).
fn compteur(v: Option<&Value>) -> Option<i64> {
    let Some(v) = v else {
        return Some(0);
    };
    if est_faux(v) {
        return Some(0);
    }
    match v {
        Value::Bool(_) => Some(1),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                return Some(i);
            }
            if n.is_u64() {
                return None;
            }
            let f = n.as_f64()?;
            let t = f.trunc();
            // 2^63 exclu : `as i64` saturerait.
            (t.is_finite()
                && (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&t))
            .then_some(t as i64)
        }
        Value::String(s) => entier_depuis_texte(s),
        _ => None,
    }
}

pub type Extrait = (Cle, Option<f64>, [i64; 3]);

/// Comme `extraire`, mais rend `Err("AttributeError")` quand `obj` n'est pas un
/// objet JSON (Python appelle `.get` dessus).
pub fn extraire_strict(obj: &Value) -> Result<Option<Extrait>, &'static str> {
    let obj = obj.as_object().ok_or("AttributeError")?;
    let Some(msg) = obj.get("message").and_then(Value::as_object) else {
        return Ok(None);
    };
    let Some(usage) = msg.get("usage").and_then(Value::as_object) else {
        return Ok(None);
    };
    let ident = match msg.get("id") {
        Some(v) if !est_faux(v) => v,
        _ => match obj.get("uuid") {
            Some(v) if !est_faux(v) => v,
            _ => return Ok(None),
        },
    };
    let mut tokens = [0i64; 3];
    for (case, champ) in tokens.iter_mut().zip(CHAMPS_COMPTES) {
        match compteur(usage.get(champ)) {
            Some(n) => *case = n,
            None => return Ok(None),
        }
    }
    let Some(ident) = en_texte(ident) else {
        return Ok(None);
    };
    let requete = match obj.get("requestId") {
        Some(v) if !est_faux(v) => match en_texte(v) {
            Some(t) => t,
            None => return Ok(None),
        },
        _ => String::new(),
    };
    let moment = obj.get("timestamp").and_then(horodatage);
    Ok(Some(((ident, requete), moment, tokens)))
}

pub fn extraire(obj: &Value) -> Option<Extrait> {
    extraire_strict(obj).ok().flatten()
}

/// Comme `modele_de`, mais rend `Err("AttributeError")` quand `obj` n'est pas un objet.
pub fn modele_de_strict(obj: &Value) -> Result<Option<String>, &'static str> {
    let obj = obj.as_object().ok_or("AttributeError")?;
    if obj.get("type").and_then(Value::as_str) != Some("assistant") {
        return Ok(None);
    }
    let Some(msg) = obj.get("message").and_then(Value::as_object) else {
        return Ok(None);
    };
    Ok(match msg.get("model").and_then(Value::as_str) {
        Some(m) if !m.is_empty() && m != "<synthetic>" => Some(m.to_string()),
        _ => None,
    })
}

pub fn modele_de(obj: &Value) -> Option<String> {
    modele_de_strict(obj).ok().flatten()
}

// --- lecture --------------------------------------------------------------

#[derive(Debug)]
struct Entree {
    horodatage: Option<f64>,
    tokens: [i64; 3],
}

/// Un transcript lu par morceaux : seules les nouvelles lignes complètes.
#[derive(Debug, Default)]
struct Fichier {
    decalage: u64,
    entrees: HashMap<Cle, Entree>,
    modele: Option<String>,
}

fn nom_de(chemin: &Path) -> String {
    chemin
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn blanc_ascii_python(c: &u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c | 0x1c..=0x1f)
}

fn contient(ligne: &[u8], aiguille: &[u8]) -> bool {
    ligne.windows(aiguille.len()).any(|w| w == aiguille)
}

impl Fichier {
    fn rafraichir(&mut self, chemin: &Path) {
        match fs::metadata(chemin) {
            Ok(m) if m.len() < self.decalage => {
                // Fichier réécrit plus court : on repart de zéro.
                *self = Fichier::default();
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(_) => {
                log::warn!("fichier illisible ignoré : {}", nom_de(chemin));
                return;
            }
        }
        if let Err(e) = self.lire(chemin)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            log::warn!("fichier illisible ignoré : {}", nom_de(chemin));
        }
    }

    /// Lit une ligne complète à la fois : jamais tout le fichier en mémoire (un
    /// transcript peut dépasser 200 Mo).
    fn lire(&mut self, chemin: &Path) -> std::io::Result<()> {
        let mut f = File::open(chemin)?;
        f.seek(SeekFrom::Start(self.decalage))?;
        let mut lecteur = BufReader::new(f);
        let mut brute = Vec::new();
        loop {
            brute.clear();
            let n = lecteur.read_until(b'\n', &mut brute)?;
            if n == 0 || brute.last() != Some(&b'\n') {
                // Fin de fichier, ou ligne incomplète : elle attend le tour suivant,
                // décalage pas avancé.
                return Ok(());
            }
            self.decalage += n as u64;
            let ligne = &brute[..n - 1];
            if ligne.iter().all(blanc_ascii_python) {
                continue;
            }
            // Filtre rapide avant le coût du parsing : la grande majorité des
            // lignes d'un transcript (tool_use, tool_result...) ne portent ni usage
            // ni modèle.
            if !contient(ligne, br#""usage""#) && !contient(ligne, br#""model""#) {
                continue;
            }
            match serde_json::from_slice::<Value>(ligne) {
                Ok(obj) if obj.is_object() => self.integrer(&obj),
                Ok(_) => {}
                Err(_) => log::warn!("ligne illisible ignorée dans {}", nom_de(chemin)),
            }
        }
    }

    fn integrer(&mut self, obj: &Value) {
        if let Some((cle, moment, tokens)) = extraire(obj) {
            match self.entrees.get_mut(&cle) {
                None => {
                    self.entrees.insert(
                        cle,
                        Entree {
                            horodatage: moment,
                            tokens,
                        },
                    );
                }
                Some(entree) => {
                    for (a, b) in entree.tokens.iter_mut().zip(tokens) {
                        *a = (*a).max(b);
                    }
                }
            }
        }
        if let Some(modele) = modele_de(obj) {
            self.modele = Some(modele);
        }
    }
}

fn total<'a>(fichiers: impl Iterator<Item = &'a Fichier>, depuis: Option<f64>) -> i64 {
    let mut fusion: HashMap<&Cle, [i64; 3]> = HashMap::new();
    for fichier in fichiers {
        for (cle, entree) in &fichier.entrees {
            if let Some(minimum) = depuis {
                match entree.horodatage {
                    Some(h) if h >= minimum => {}
                    _ => continue,
                }
            }
            let courant = fusion.entry(cle).or_insert([0; 3]);
            for (a, b) in courant.iter_mut().zip(entree.tokens) {
                *a = (*a).max(b);
            }
        }
    }
    fusion
        .values()
        .flat_map(|t| t.iter())
        .fold(0i64, |s, v| s.saturating_add(*v))
}

pub struct SuiviTokens {
    racine: PathBuf,
    fichiers: HashMap<PathBuf, Fichier>,
}

impl SuiviTokens {
    pub fn new(racine: PathBuf) -> Self {
        Self {
            racine,
            fichiers: HashMap::new(),
        }
    }

    pub fn fichiers_suivis(&self) -> HashSet<PathBuf> {
        self.fichiers.keys().cloned().collect()
    }

    pub fn rafraichir(&mut self, transcripts: &[PathBuf], minuit: f64) {
        let mut voulus: HashSet<PathBuf> = HashSet::new();
        for transcript in transcripts {
            voulus.extend(fichiers_de_session(transcript));
        }
        let mut candidats = Vec::new();
        parcourir(&self.racine, &mut candidats);
        for chemin in candidats {
            match fs::metadata(&chemin).and_then(|m| m.modified()) {
                Ok(t) => {
                    let mtime = match t.duration_since(UNIX_EPOCH) {
                        Ok(d) => d.as_secs_f64(),
                        Err(e) => -e.duration().as_secs_f64(),
                    };
                    if mtime >= minuit {
                        voulus.insert(chemin);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => log::warn!("fichier illisible ignoré : {}", nom_de(&chemin)),
            }
        }
        self.fichiers.retain(|chemin, _| voulus.contains(chemin));
        for chemin in voulus {
            self.fichiers
                .entry(chemin.clone())
                .or_default()
                .rafraichir(&chemin);
        }
    }

    pub fn tokens_session(&self, transcript: &Path) -> i64 {
        let chemins = fichiers_de_session(transcript);
        total(chemins.iter().filter_map(|p| self.fichiers.get(p)), None)
    }

    pub fn tokens_jour(&self, minuit: f64) -> i64 {
        total(self.fichiers.values(), Some(minuit))
    }

    pub fn modele(&self, transcript: &Path) -> Option<String> {
        self.fichiers.get(transcript)?.modele.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, LocalResult, NaiveDateTime, Utc};
    use serde_json::json;
    use std::io::Write;
    use std::time::SystemTime;

    fn maintenant() -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0)
    }

    fn iso(t: f64) -> String {
        let d = Local.timestamp_opt(t.floor() as i64, 0).unwrap();
        d.format("%Y-%m-%dT%H:%M:%S%.6f%:z").to_string()
    }

    struct Ligne {
        id: &'static str,
        req: &'static str,
        entree: i64,
        sortie: i64,
        ecriture: i64,
        lecture: i64,
        t: Option<f64>,
        modele: &'static str,
        typ: &'static str,
    }

    fn ligne(id: &'static str) -> Ligne {
        Ligne {
            id,
            req: "r1",
            entree: 0,
            sortie: 0,
            ecriture: 0,
            lecture: 0,
            t: None,
            modele: "claude-opus-5",
            typ: "assistant",
        }
    }

    impl Ligne {
        fn texte(&self, maintenant: f64) -> String {
            let v = json!({
                "type": self.typ,
                "requestId": self.req,
                "timestamp": iso(self.t.unwrap_or(maintenant)),
                "message": {
                    "id": self.id,
                    "model": self.modele,
                    "usage": {
                        "input_tokens": self.entree,
                        "output_tokens": self.sortie,
                        "cache_creation_input_tokens": self.ecriture,
                        "cache_read_input_tokens": self.lecture,
                    },
                },
            });
            format!("{v}\n")
        }
    }

    struct Cadre {
        maintenant: f64,
        minuit: f64,
    }

    fn cadre() -> Cadre {
        let maintenant = maintenant();
        Cadre {
            maintenant,
            minuit: minuit_local(maintenant),
        }
    }

    impl Cadre {
        fn l(&self, l: Ligne) -> String {
            l.texte(self.maintenant)
        }
    }

    fn session(
        racine: &Path,
        sid: &str,
        lignes: &[String],
        sous_agents: &[(&str, Vec<String>)],
    ) -> PathBuf {
        let t = racine.join("projet").join(format!("{sid}.jsonl"));
        fs::create_dir_all(t.parent().unwrap()).unwrap();
        fs::write(&t, lignes.concat()).unwrap();
        for (nom, contenu) in sous_agents {
            let f = racine
                .join("projet")
                .join(sid)
                .join("subagents")
                .join(format!("{nom}.jsonl"));
            fs::create_dir_all(f.parent().unwrap()).unwrap();
            fs::write(&f, contenu.concat()).unwrap();
        }
        t
    }

    fn ajouter(chemin: &Path, texte: &str) {
        let mut f = fs::OpenOptions::new().append(true).open(chemin).unwrap();
        f.write_all(texte.as_bytes()).unwrap();
    }

    #[test]
    fn test_exclut_les_lectures_du_cache() {
        let c = cadre();
        let d = tempfile::tempdir().unwrap();
        let t = session(
            d.path(),
            "s1",
            &[c.l(Ligne {
                entree: 10,
                sortie: 5,
                ecriture: 100,
                lecture: 10_000,
                ..ligne("m1")
            })],
            &[],
        );
        let mut suivi = SuiviTokens::new(d.path().to_path_buf());
        suivi.rafraichir(std::slice::from_ref(&t), c.minuit);
        assert_eq!(suivi.tokens_session(&t), 115);
    }

    #[test]
    fn test_doublons_a_valeurs_differentes_gardent_le_maximum() {
        let c = cadre();
        let d = tempfile::tempdir().unwrap();
        let t = session(
            d.path(),
            "s1",
            &[
                c.l(Ligne {
                    entree: 10,
                    sortie: 5,
                    ..ligne("m1")
                }),
                c.l(Ligne {
                    entree: 10,
                    sortie: 50,
                    ..ligne("m1")
                }),
            ],
            &[],
        );
        let mut suivi = SuiviTokens::new(d.path().to_path_buf());
        suivi.rafraichir(std::slice::from_ref(&t), c.minuit);
        assert_eq!(suivi.tokens_session(&t), 60);
    }

    #[test]
    fn test_sous_agents_inclus_et_dedoublonnes_entre_fichiers() {
        let c = cadre();
        let d = tempfile::tempdir().unwrap();
        let t = session(
            d.path(),
            "s1",
            &[c.l(Ligne {
                sortie: 10,
                ..ligne("m1")
            })],
            &[(
                "agent-a",
                vec![
                    c.l(Ligne {
                        sortie: 20,
                        ..ligne("m2")
                    }),
                    c.l(Ligne {
                        sortie: 10,
                        ..ligne("m1")
                    }),
                ],
            )],
        );
        let mut suivi = SuiviTokens::new(d.path().to_path_buf());
        suivi.rafraichir(std::slice::from_ref(&t), c.minuit);
        assert_eq!(suivi.tokens_session(&t), 30);
    }

    #[test]
    fn test_ligne_incomplete_attend_le_tour_suivant() {
        let c = cadre();
        let d = tempfile::tempdir().unwrap();
        let t = session(
            d.path(),
            "s1",
            &[c.l(Ligne {
                sortie: 10,
                ..ligne("m1")
            })],
            &[],
        );
        ajouter(
            &t,
            c.l(Ligne {
                sortie: 7,
                ..ligne("m2")
            })
            .trim_end_matches('\n'),
        );
        let mut suivi = SuiviTokens::new(d.path().to_path_buf());
        suivi.rafraichir(std::slice::from_ref(&t), c.minuit);
        assert_eq!(suivi.tokens_session(&t), 10);
        ajouter(&t, "\n");
        suivi.rafraichir(std::slice::from_ref(&t), c.minuit);
        assert_eq!(suivi.tokens_session(&t), 17);
    }

    #[test]
    fn test_ligne_illisible_ignoree() {
        let c = cadre();
        let d = tempfile::tempdir().unwrap();
        let t = session(
            d.path(),
            "s1",
            &[
                "{pas du json\n".to_string(),
                c.l(Ligne {
                    sortie: 3,
                    ..ligne("m1")
                }),
            ],
            &[],
        );
        let mut suivi = SuiviTokens::new(d.path().to_path_buf());
        suivi.rafraichir(std::slice::from_ref(&t), c.minuit);
        assert_eq!(suivi.tokens_session(&t), 3);
    }

    #[test]
    fn test_fichier_reecrit_plus_court_relu_depuis_le_debut() {
        let c = cadre();
        let d = tempfile::tempdir().unwrap();
        let t = session(
            d.path(),
            "s1",
            &[
                c.l(Ligne {
                    sortie: 10,
                    ..ligne("m1")
                }),
                c.l(Ligne {
                    sortie: 10,
                    ..ligne("m2")
                }),
            ],
            &[],
        );
        let mut suivi = SuiviTokens::new(d.path().to_path_buf());
        suivi.rafraichir(std::slice::from_ref(&t), c.minuit);
        fs::write(
            &t,
            c.l(Ligne {
                sortie: 4,
                ..ligne("m3")
            }),
        )
        .unwrap();
        suivi.rafraichir(std::slice::from_ref(&t), c.minuit);
        assert_eq!(suivi.tokens_session(&t), 4);
    }

    #[test]
    fn test_cumul_du_jour_commence_a_minuit() {
        let c = cadre();
        let d = tempfile::tempdir().unwrap();
        let hier = c.minuit - 3600.0;
        let t = session(
            d.path(),
            "s1",
            &[
                c.l(Ligne {
                    sortie: 100,
                    t: Some(hier),
                    ..ligne("m1")
                }),
                c.l(Ligne {
                    sortie: 5,
                    ..ligne("m2")
                }),
            ],
            &[],
        );
        let autre = session(
            d.path(),
            "s2",
            &[c.l(Ligne {
                sortie: 7,
                ..ligne("m3")
            })],
            &[],
        );
        let mut suivi = SuiviTokens::new(d.path().to_path_buf());
        suivi.rafraichir(std::slice::from_ref(&t), c.minuit);
        assert_eq!(suivi.tokens_session(&t), 105);
        // m2 + m3, s2 trouvé par sa date de modification.
        assert_eq!(suivi.tokens_jour(c.minuit), 12);
        assert!(suivi.fichiers_suivis().contains(&autre));
    }

    #[test]
    fn test_fichier_ancien_hors_session_oublie() {
        let c = cadre();
        let d = tempfile::tempdir().unwrap();
        let t = session(
            d.path(),
            "s1",
            &[c.l(Ligne {
                sortie: 1,
                t: Some(c.minuit - 86_400.0),
                ..ligne("m1")
            })],
            &[],
        );
        let vieux = SystemTime::now()
            - std::time::Duration::from_secs_f64(c.maintenant - c.minuit + 86_400.0);
        File::options()
            .write(true)
            .open(&t)
            .unwrap()
            .set_modified(vieux)
            .unwrap();
        let mut suivi = SuiviTokens::new(d.path().to_path_buf());
        suivi.rafraichir(std::slice::from_ref(&t), c.minuit);
        assert!(suivi.fichiers_suivis().contains(&t));
        suivi.rafraichir(&[], c.minuit);
        assert!(!suivi.fichiers_suivis().contains(&t));
    }

    #[test]
    fn test_modele_du_dernier_message_assistant() {
        let c = cadre();
        let d = tempfile::tempdir().unwrap();
        let t = session(
            d.path(),
            "s1",
            &[
                c.l(Ligne {
                    modele: "claude-sonnet-5",
                    ..ligne("m1")
                }),
                c.l(Ligne {
                    modele: "claude-opus-5",
                    ..ligne("m2")
                }),
                c.l(Ligne {
                    modele: "<synthetic>",
                    ..ligne("m3")
                }),
                c.l(Ligne {
                    modele: "claude-haiku-4-5",
                    typ: "user",
                    ..ligne("m4")
                }),
            ],
            &[],
        );
        let mut suivi = SuiviTokens::new(d.path().to_path_buf());
        suivi.rafraichir(std::slice::from_ref(&t), c.minuit);
        assert_eq!(suivi.modele(&t).as_deref(), Some("claude-opus-5"));
    }

    #[test]
    fn test_session_inconnue_vaut_zero() {
        let d = tempfile::tempdir().unwrap();
        let suivi = SuiviTokens::new(d.path().to_path_buf());
        assert_eq!(suivi.tokens_session(&d.path().join("absent.jsonl")), 0);
        assert_eq!(suivi.modele(&d.path().join("absent.jsonl")), None);
    }

    #[test]
    fn test_fichiers_de_session() {
        let d = tempfile::tempdir().unwrap();
        let t = session(
            d.path(),
            "s1",
            &[],
            &[("agent-b", vec![]), ("agent-a", vec![])],
        );
        let dossier = d.path().join("projet").join("s1").join("subagents");
        assert_eq!(
            fichiers_de_session(&t),
            vec![
                t.clone(),
                dossier.join("agent-a.jsonl"),
                dossier.join("agent-b.jsonl")
            ]
        );
    }

    #[test]
    fn test_minuit_local() {
        let c = cadre();
        let d = Local.timestamp_opt(c.minuit as i64, 0).unwrap();
        assert_eq!(d.format("%H:%M:%S").to_string(), "00:00:00");
        let ecart = c.maintenant - c.minuit;
        assert!((0.0..25.0 * 3600.0).contains(&ecart));
    }

    /// Europe/Paris sans base de fuseaux : CET (UTC+1), CEST (UTC+2) du dernier
    /// dimanche de mars au dernier dimanche d'octobre, à 01:00 UTC. Évite de fixer
    /// `TZ` dans un test (variable d'environnement partagée par tout le processus).
    #[derive(Clone, Copy)]
    struct ParisFictif;

    fn dernier_dimanche(annee: i32, mois: u32) -> chrono::NaiveDate {
        use chrono::{Datelike, Weekday};
        let mut j = NaiveDate::from_ymd_opt(annee, mois, 31).unwrap();
        while j.weekday() != Weekday::Sun {
            j = j.pred_opt().unwrap();
        }
        j
    }

    fn decalage_paris(utc: &NaiveDateTime) -> FixedOffset {
        use chrono::Datelike;
        let debut = dernier_dimanche(utc.year(), 3)
            .and_hms_opt(1, 0, 0)
            .unwrap();
        let fin = dernier_dimanche(utc.year(), 10)
            .and_hms_opt(1, 0, 0)
            .unwrap();
        let s = if *utc >= debut && *utc < fin {
            7200
        } else {
            3600
        };
        FixedOffset::east_opt(s).unwrap()
    }

    impl TimeZone for ParisFictif {
        type Offset = FixedOffset;
        fn from_offset(_: &FixedOffset) -> Self {
            ParisFictif
        }
        fn offset_from_local_date(&self, _: &chrono::NaiveDate) -> LocalResult<FixedOffset> {
            LocalResult::None
        }
        fn offset_from_local_datetime(&self, local: &NaiveDateTime) -> LocalResult<FixedOffset> {
            let a = decalage_paris(&(*local - Duration::seconds(3600)));
            let b = decalage_paris(&(*local - Duration::seconds(7200)));
            let valide = |o: FixedOffset| {
                decalage_paris(&(*local - Duration::seconds(i64::from(o.local_minus_utc())))) == o
            };
            match (valide(a), valide(b)) {
                (true, true) if a == b => LocalResult::Single(a),
                (true, true) => LocalResult::Ambiguous(b, a),
                (true, false) => LocalResult::Single(a),
                (false, true) => LocalResult::Single(b),
                (false, false) => LocalResult::None,
            }
        }
        fn offset_from_utc_date(&self, utc: &chrono::NaiveDate) -> FixedOffset {
            decalage_paris(&utc.and_hms_opt(12, 0, 0).unwrap())
        }
        fn offset_from_utc_datetime(&self, utc: &NaiveDateTime) -> FixedOffset {
            decalage_paris(utc)
        }
    }

    fn utc(a: i32, m: u32, j: u32, h: u32) -> f64 {
        Utc.with_ymd_and_hms(a, m, j, h, 0, 0).unwrap().timestamp() as f64
    }

    fn paris(a: i32, m: u32, j: u32, h: u32) -> f64 {
        // Heure locale Paris -> instant, via le fuseau fictif.
        ParisFictif
            .from_local_datetime(
                &NaiveDate::from_ymd_opt(a, m, j)
                    .unwrap()
                    .and_hms_opt(h, 0, 0)
                    .unwrap(),
            )
            .earliest()
            .unwrap()
            .timestamp() as f64
    }

    #[test]
    fn test_minuit_local_passage_heure_ete() {
        // 2026-03-29 : passage à l'heure d'été à 02h00 -> 03h00 local. Minuit est
        // encore à l'ancien décalage (CET, UTC+1), pas celui de "maintenant" (CEST, UTC+2).
        let maintenant = paris(2026, 3, 29, 14);
        assert_eq!(
            minuit_selon(&ParisFictif, maintenant),
            Ok(utc(2026, 3, 28, 23))
        );
    }

    #[test]
    fn test_minuit_local_passage_heure_hiver() {
        // 2026-10-25 : passage à l'heure d'hiver à 03h00 -> 02h00 local. Minuit est
        // encore à l'ancien décalage (CEST, UTC+2), pas celui de "maintenant" (CET, UTC+1).
        let maintenant = paris(2026, 10, 25, 14);
        assert_eq!(
            minuit_selon(&ParisFictif, maintenant),
            Ok(utc(2026, 10, 24, 22))
        );
    }

    #[test]
    fn test_minuit_local_jour_ordinaire_tz_paris() {
        let maintenant = paris(2026, 1, 15, 10);
        assert_eq!(
            minuit_selon(&ParisFictif, maintenant),
            Ok(utc(2026, 1, 14, 23))
        );
    }

    // Chemin `Local` sous un vrai fuseau : le binaire de test se relance dans un
    // sous-processus avec `TZ` (aucun `set_var` dans le processus de test).
    fn relancer_sous_tz(tz: &str, enfant: &str) {
        let sortie = std::process::Command::new(std::env::current_exe().unwrap())
            .env("TZ", tz)
            .args(["--exact", enfant, "--ignored", "--nocapture"])
            .output()
            .unwrap();
        assert!(
            sortie.status.success(),
            "{}{}",
            String::from_utf8_lossy(&sortie.stdout),
            String::from_utf8_lossy(&sortie.stderr)
        );
        assert!(String::from_utf8_lossy(&sortie.stdout).contains("1 passed"));
    }

    #[test]
    fn test_minuit_local_ambigu_sous_tz_havane() {
        relancer_sous_tz(
            "America/Havana",
            "tokens::tests::enfant_minuit_ambigu_havane",
        );
    }

    #[test]
    #[ignore = "lancé en sous-processus avec TZ=America/Havana"]
    fn enfant_minuit_ambigu_havane() {
        // 2010-10-31 : repli de 01:00 à 00:00, minuit existe deux fois. Python (fold=0)
        // prend le premier minuit (UTC-4).
        assert_eq!(minuit_local(1288501200.0 + 3600.0 * 5.0), 1288497600.0);
    }

    #[test]
    fn test_minuit_local_inexistant_sous_tz_sao_paulo() {
        relancer_sous_tz(
            "America/Sao_Paulo",
            "tokens::tests::enfant_minuit_inexistant_sao_paulo",
        );
    }

    #[test]
    #[ignore = "lancé en sous-processus avec TZ=America/Sao_Paulo"]
    fn enfant_minuit_inexistant_sao_paulo() {
        // 2018-11-04 : minuit n'existe pas (00:00 -> 01:00). Valeurs relevées avec Python
        // sous le même TZ.
        assert_eq!(minuit_local(1541340000.0), 1541300400.0);
        assert_eq!(minuit_local(1536505200.0), 1536462000.0);
    }

    #[test]
    fn test_minuit_local_exceptions_python() {
        assert_eq!(minuit_local_strict(f64::NAN), Err("ValueError"));
        assert_eq!(minuit_local_strict(f64::INFINITY), Err("OverflowError"));
        assert_eq!(minuit_local_strict(1e21), Err("OverflowError"));
        assert_eq!(
            minuit_local_strict(9_007_199_254_740_992.0),
            Err("ValueError")
        );
        assert!(minuit_local_strict(0.0).is_ok());
    }

    // --- lecture ligne à ligne, sans liste intermédiaire ---

    #[test]
    fn test_beaucoup_de_lignes_totaux_corrects_et_ligne_finale_incomplete_pas_consommee() {
        let c = cadre();
        let d = tempfile::tempdir().unwrap();
        let ids: Vec<&'static str> = (0..2000)
            .map(|i| &*Box::leak(format!("m{i}").into_boxed_str()))
            .collect();
        let lignes: Vec<String> = ids
            .iter()
            .map(|i| {
                c.l(Ligne {
                    sortie: 1,
                    ..ligne(i)
                })
            })
            .collect();
        let t = session(d.path(), "s1", &lignes, &[]);
        ajouter(
            &t,
            c.l(Ligne {
                sortie: 99,
                ..ligne("mfinal")
            })
            .trim_end_matches('\n'),
        );
        let mut suivi = SuiviTokens::new(d.path().to_path_buf());
        suivi.rafraichir(std::slice::from_ref(&t), c.minuit);
        assert_eq!(suivi.tokens_session(&t), 2000);
        ajouter(&t, "\n");
        suivi.rafraichir(std::slice::from_ref(&t), c.minuit);
        assert_eq!(suivi.tokens_session(&t), 2099);
    }

    #[test]
    fn test_reveillon_meme_suivi_tokens_change_de_jour_entre_deux_rafraichissements() {
        let c = cadre();
        let d = tempfile::tempdir().unwrap();
        let hier_minuit = c.minuit - 86_400.0;
        let t = session(
            d.path(),
            "s1",
            &[
                c.l(Ligne {
                    sortie: 10,
                    t: Some(c.minuit - 3600.0),
                    ..ligne("m1")
                }),
                c.l(Ligne {
                    sortie: 20,
                    t: Some(c.minuit + 3600.0),
                    ..ligne("m2")
                }),
            ],
            &[],
        );
        let mut suivi = SuiviTokens::new(d.path().to_path_buf());
        suivi.rafraichir(std::slice::from_ref(&t), hier_minuit);
        assert_eq!(suivi.tokens_jour(hier_minuit), 30);
        // Même objet vivant, la journée a tourné.
        suivi.rafraichir(std::slice::from_ref(&t), c.minuit);
        assert_eq!(suivi.tokens_jour(c.minuit), 20);
        assert_eq!(suivi.tokens_jour(hier_minuit), 30);
    }

    #[test]
    fn test_fichier_illisible_ignore_les_autres_restent_comptes() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            return; // chmod sans effet pour root
        }
        let c = cadre();
        let d = tempfile::tempdir().unwrap();
        let lisible = session(
            d.path(),
            "s1",
            &[c.l(Ligne {
                sortie: 5,
                ..ligne("m1")
            })],
            &[],
        );
        let illisible = session(
            d.path(),
            "s2",
            &[c.l(Ligne {
                sortie: 9,
                ..ligne("m2")
            })],
            &[],
        );
        fs::set_permissions(&illisible, fs::Permissions::from_mode(0o000)).unwrap();
        let mut suivi = SuiviTokens::new(d.path().to_path_buf());
        suivi.rafraichir(&[lisible.clone(), illisible.clone()], c.minuit);
        let attendu = suivi.tokens_session(&lisible);
        fs::set_permissions(&illisible, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(attendu, 5);
    }

    // --- ajouts propres au port ---

    #[test]
    fn parcours_ne_suit_pas_les_dossiers_lies() {
        let c = cadre();
        let d = tempfile::tempdir().unwrap();
        let hors = tempfile::tempdir().unwrap();
        fs::write(
            hors.path().join("x.jsonl"),
            c.l(Ligne {
                sortie: 1,
                ..ligne("mx")
            }),
        )
        .unwrap();
        let racine = d.path().join("racine");
        fs::create_dir_all(&racine).unwrap();
        std::os::unix::fs::symlink(hors.path(), racine.join("lien")).unwrap();
        let cible = session(
            d.path(),
            "s9",
            &[c.l(Ligne {
                sortie: 2,
                ..ligne("my")
            })],
            &[],
        );
        std::os::unix::fs::symlink(&cible, racine.join("fichier.jsonl")).unwrap();
        let mut suivi = SuiviTokens::new(racine.clone());
        suivi.rafraichir(&[], c.minuit);
        let suivis = suivi.fichiers_suivis();
        assert!(suivis.contains(&racine.join("fichier.jsonl")));
        assert!(!suivis.iter().any(|p| p.ends_with("x.jsonl")));
    }

    #[test]
    fn compteurs_comme_int_de_python() {
        let attendu = |v: Value| compteur(Some(&v));
        assert_eq!(attendu(json!(7)), Some(7));
        assert_eq!(attendu(json!(7.9)), Some(7));
        assert_eq!(attendu(json!(-7.9)), Some(-7));
        assert_eq!(attendu(json!(true)), Some(1));
        assert_eq!(attendu(json!(false)), Some(0));
        assert_eq!(attendu(json!(" 1_000 ")), Some(1000));
        assert_eq!(attendu(json!("-5")), Some(-5));
        assert_eq!(attendu(json!("+5")), Some(5));
        assert_eq!(attendu(json!("")), Some(0));
        assert_eq!(attendu(json!(null)), Some(0));
        assert_eq!(attendu(json!([])), Some(0));
        assert_eq!(attendu(json!({})), Some(0));
        assert_eq!(attendu(json!("1__0")), None);
        assert_eq!(attendu(json!("_1")), None);
        assert_eq!(attendu(json!("1_")), None);
        assert_eq!(attendu(json!("abc")), None);
        assert_eq!(attendu(json!(" ")), None);
        assert_eq!(attendu(json!([1])), None);
        assert_eq!(attendu(json!({"a": 1})), None);
        assert_eq!(attendu(json!(1e30)), None);
        assert_eq!(attendu(json!(u64::MAX)), None);
    }

    #[test]
    fn extraire_identifiants_et_exceptions() {
        let base = |id: Value, uuid: Value, req: Value| json!({"uuid": uuid, "requestId": req, "message": {"id": id, "usage": {"output_tokens": 1}}});
        let cle = |v: Value| extraire(&v).map(|e| e.0);
        assert_eq!(
            cle(base(json!("m"), json!(null), json!(null))),
            Some(("m".into(), String::new()))
        );
        assert_eq!(
            cle(base(json!(""), json!("u"), json!(0))),
            Some(("u".into(), String::new()))
        );
        assert_eq!(
            cle(base(json!(5), json!(null), json!(true))),
            Some(("5".into(), "True".into()))
        );
        assert_eq!(
            cle(base(json!(true), json!(null), json!(null))),
            Some(("True".into(), String::new()))
        );
        assert_eq!(cle(base(json!(0), json!(null), json!(null))), None);
        assert_eq!(cle(base(json!(1.5), json!(null), json!(null))), None);
        assert_eq!(cle(base(json!("m"), json!(null), json!([1]))), None);
        assert_eq!(extraire_strict(&json!("x")), Err("AttributeError"));
        assert_eq!(modele_de_strict(&json!([])), Err("AttributeError"));
        assert_eq!(
            extraire(&json!({"message": {"id": "m", "usage": {"input_tokens": "x"}}})),
            None
        );
    }

    #[test]
    fn horodatage_formats_acceptes() {
        let h = |s: &str| horodatage(&json!(s));
        assert_eq!(
            h("2026-11-20T20:05:25.931211+01:00"),
            Some(1795201525.931211)
        );
        assert_eq!(h("2026-11-20T19:05:25Z"), Some(1795201525.0));
        assert_eq!(h("2026-11-20 20:05:25-00:00"), Some(1795205125.0));
        assert!(h("2026-11-20T20:05:25").is_some());
        assert!(h("2026-11-20T20:05:25.5").is_some());
        assert!(h("2026-11-20").is_some());
        assert_eq!(h("20261120T200525"), None);
        assert_eq!(h("2026-13-20"), None);
        assert_eq!(h("2026-11-20T25:00:00Z"), None);
        assert_eq!(h(" 2026-11-20"), None);
        assert_eq!(horodatage(&json!(5)), None);
    }
}

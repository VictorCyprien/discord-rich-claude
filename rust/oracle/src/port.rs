//! The Rust port of `src/discord_rich_claude/`.
//!
//! Replace each `Err("non porté".into())` with the port of the source function. Return `Err` where
//! the source throws or raises: the caller gets an error there too. After changing a
//! signature, run `rust-anything refresh` to regenerate the bindings. `rust-anything diff` checks
//! behavior against what the source recorded.

#![forbid(unsafe_code)]

/// Port of `formater_nombre` (`src/discord_rich_claude/carte.py:31`).
pub fn formater_nombre(n: i64) -> Result<String, String> {
    Ok(discord_rich_claude::carte::formater_nombre(n))
}

/// Port of `nom_modele` (`src/discord_rich_claude/carte.py:39`).
pub fn nom_modele(model_id: Option<String>) -> Result<Option<String>, String> {
    Ok(discord_rich_claude::carte::nom_modele(model_id.as_deref()))
}

/// Port of `est_vue_b` (`src/discord_rich_claude/carte.py:80`).
pub fn est_vue_b(maintenant: f64, rotation_s: f64) -> Result<bool, String> {
    discord_rich_claude::carte::est_vue_b_strict(maintenant, rotation_s)
        .map_err(|nom| format!("{nom}: division impossible"))
}

/// Classe d'exception que Python lève sur un `settings` qui n'est pas un objet
/// (`"hooks" in settings`, puis `settings["hooks"]` ou `settings.get`).
/// `None` : le cas se traite normalement (objet).
fn erreur_forme(settings: &serde_json::Value, avec_get: bool) -> Option<Result<(), String>> {
    use serde_json::Value;
    let contient_hooks = match settings {
        Value::Object(_) => return None,
        Value::String(t) => t.contains("hooks"),
        Value::Array(v) => v.iter().any(|e| e.as_str() == Some("hooks")),
        _ => {
            return Some(Err(
                "TypeError: argument of type is not a container or iterable".into(),
            ));
        }
    };
    if contient_hooks {
        Some(Err("TypeError: indices must be integers".into()))
    } else if avec_get {
        Some(Err("AttributeError: object has no attribute 'get'".into()))
    } else {
        Some(Ok(()))
    }
}

/// Port of `ajouter_hooks` (`src/discord_rich_claude/installation.py:53`).
///
/// `commande` defaults to `COMMANDE_HOOK` in the source.
pub fn ajouter_hooks(
    settings: serde_json::Value,
    commande: Option<String>,
) -> Result<serde_json::Value, String> {
    if let Some(erreur) = erreur_forme(&settings, true) {
        return erreur.map(|()| settings);
    }
    let commande =
        commande.unwrap_or_else(|| discord_rich_claude::chemins::COMMANDE_HOOK.to_string());
    discord_rich_claude::installation::ajouter_hooks(&settings, &commande)
        .map_err(|m| format!("ValueError: {m}"))
}

/// Port of `retirer_hooks` (`src/discord_rich_claude/installation.py:71`).
pub fn retirer_hooks(settings: serde_json::Value) -> Result<serde_json::Value, String> {
    if let Some(erreur) = erreur_forme(&settings, false) {
        return erreur.map(|()| settings);
    }
    discord_rich_claude::installation::retirer_hooks(&settings)
        .map_err(|m| format!("ValueError: {m}"))
}

/// Port of `filtrer` (`src/discord_rich_claude/serveur.py:87`).
pub fn filtrer(
    corps: serde_json::Value,
    pid_brut: Option<String>,
    recu_a: f64,
    projets_claude: serde_json::Value,
) -> Result<Option<serde_json::Value>, String> {
    use serde_json::{Value, json};
    // `json.loads` accepte du texte ou des octets, rien d'autre.
    let octets: Vec<u8> = match &corps {
        Value::String(texte) => texte.as_bytes().to_vec(),
        Value::Object(o) if o.len() == 1 && o.contains_key("$bytes") => o["$bytes"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_u64)
                    .map(|b| b as u8)
                    .collect()
            })
            .unwrap_or_default(),
        autre => {
            let nom = match autre {
                Value::Null => "NoneType",
                Value::Bool(_) => "bool",
                Value::Number(n) if n.is_f64() => "float",
                Value::Number(_) => "int",
                Value::Array(_) => "list",
                _ => "dict",
            };
            return Err(format!(
                "TypeError: the JSON object must be str, bytes or bytearray, not {nom}"
            ));
        }
    };
    // la racine ne sert qu'aux transcripts ; un type inattendu n'en autorise aucun
    let racine = std::path::PathBuf::from(projets_claude.as_str().unwrap_or("\0invalide"));
    let evenement =
        discord_rich_claude::serveur::filtrer(&octets, pid_brut.as_deref(), recu_a, &racine);
    Ok(evenement.map(|ev| {
        json!({"$object": "Evenement", "attrs": {
            "nom": ev.nom,
            "session_id": ev.session_id,
            "recu_a": {"$float": format!("{:?}", ev.recu_a)},
            "cwd": ev.cwd,
            "tool_name": ev.tool_name,
            "agent_id": ev.agent_id,
            "transcript_path": ev.transcript_path,
            "pid": ev.pid,
        }})
    }))
}

/// Port of `libelle_outil` (`src/discord_rich_claude/sessions.py:53`).
pub fn libelle_outil(tool_name: Option<String>) -> Result<String, String> {
    Ok(discord_rich_claude::sessions::libelle_outil(tool_name.as_deref()).to_string())
}

/// Port of `minuit_local` (`src/discord_rich_claude/tokens.py:29`).
pub fn minuit_local(maintenant: f64) -> Result<f64, String> {
    discord_rich_claude::tokens::minuit_local_strict(maintenant)
        .map_err(|nom| format!("{nom}: horodatage hors limites"))
}

/// Port of `extraire` (`src/discord_rich_claude/tokens.py:51`).
pub fn extraire(obj: serde_json::Value) -> Result<Option<serde_json::Value>, String> {
    let extrait = discord_rich_claude::tokens::extraire_strict(&obj)
        .map_err(|nom| format!("{nom}: 'objet' sans attribut 'get'"))?;
    Ok(extrait.map(|((id, requete), moment, tokens)| {
        let champs: serde_json::Map<String, serde_json::Value> =
            discord_rich_claude::tokens::CHAMPS_COMPTES
                .iter()
                .zip(tokens)
                .map(|(k, v)| ((*k).to_string(), serde_json::Value::from(v)))
                .collect();
        serde_json::json!({"$tuple": [{"$tuple": [id, requete]}, moment, champs]})
    }))
}

/// Port of `modele_de` (`src/discord_rich_claude/tokens.py:67`).
pub fn modele_de(obj: serde_json::Value) -> Result<Option<String>, String> {
    discord_rich_claude::tokens::modele_de_strict(&obj)
        .map_err(|nom| format!("{nom}: 'objet' sans attribut 'get'"))
}

//! Python bindings for `src/discord_rich_claude/`: generated from the signatures in `port.rs` by rust-anything.
//! Regenerate with `rust-anything refresh` after changing them.

use pyo3::prelude::*;

pub mod js;
pub mod port;
pub mod pyconv;
pub mod pynum;

#[allow(unused_imports)]
use pynum::Num;

fn error(message: String) -> PyErr {
    use pyo3::exceptions as e;
    let (kind, text) = message.split_once(": ").unwrap_or(("", message.as_str()));
    let text = text.to_string();
    match kind {
        "TypeError" => e::PyTypeError::new_err(text),
        "KeyError" => e::PyKeyError::new_err(text),
        "IndexError" => e::PyIndexError::new_err(text),
        "OverflowError" => e::PyOverflowError::new_err(text),
        "ZeroDivisionError" => e::PyZeroDivisionError::new_err(text),
        "AttributeError" => e::PyAttributeError::new_err(text),
        "RuntimeError" => e::PyRuntimeError::new_err(text),
        "NotImplementedError" => e::PyNotImplementedError::new_err(text),
        "LookupError" => e::PyLookupError::new_err(text),
        "ArithmeticError" => e::PyArithmeticError::new_err(text),
        "UnicodeError" => e::PyUnicodeError::new_err(text),
        "ValueError" => e::PyValueError::new_err(text),
        _ => e::PyValueError::new_err(message),
    }
}

fn formater_nombre_inner(n: i64) -> Result<String, String> {
    let out = port::formater_nombre(n)?;
    Ok(out)
}

/// `formater_nombre`.
#[pyfunction]
#[pyo3(name = "formater_nombre", signature = (n))]
fn formater_nombre(n: i64) -> PyResult<String> {
    formater_nombre_inner(n).map_err(error)
}

fn nom_modele_inner(model_id: Option<String>) -> Result<Option<String>, String> {
    let out = port::nom_modele(model_id)?;
    Ok(out)
}

/// `nom_modele`.
#[pyfunction]
#[pyo3(name = "nom_modele", signature = (model_id))]
fn nom_modele(model_id: Option<String>) -> PyResult<Option<String>> {
    nom_modele_inner(model_id).map_err(error)
}

fn nom_projet_inner(cwd: Option<String>, autorises: Bound<'_, PyAny>) -> Result<String, String> {
    let autorises: serde_json::Value = crate::pyconv::FromPy::from_py(&autorises)?;
    let out = port::nom_projet(cwd, autorises)?;
    Ok(out)
}

/// `nom_projet`.
#[pyfunction]
#[pyo3(name = "nom_projet", signature = (cwd, autorises))]
fn nom_projet(cwd: Option<String>, autorises: Bound<'_, PyAny>) -> PyResult<String> {
    nom_projet_inner(cwd, autorises).map_err(error)
}

fn est_vue_b_inner(maintenant: f64, rotation_s: f64) -> Result<bool, String> {
    let out = port::est_vue_b(maintenant, rotation_s)?;
    Ok(out)
}

/// `est_vue_b`.
#[pyfunction]
#[pyo3(name = "est_vue_b", signature = (maintenant, rotation_s))]
fn est_vue_b(maintenant: f64, rotation_s: f64) -> PyResult<bool> {
    est_vue_b_inner(maintenant, rotation_s).map_err(error)
}

fn charger_inner(chemin: Bound<'_, PyAny>) -> Result<serde_json::Value, String> {
    let chemin: serde_json::Value = crate::pyconv::FromPy::from_py(&chemin)?;
    let out = port::charger(chemin)?;
    Ok(out)
}

/// `charger`.
#[pyfunction]
#[pyo3(name = "charger", signature = (chemin))]
fn charger(py: Python<'_>, chemin: Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
    charger_inner(chemin)
        .map_err(error)
        .and_then(|v| crate::pyconv::ToPy::to_py(&v, py))
}

fn ajouter_hooks_inner(
    settings: Bound<'_, PyAny>,
    commande: Option<String>,
) -> Result<serde_json::Value, String> {
    let settings: serde_json::Value = crate::pyconv::FromPy::from_py(&settings)?;
    let out = port::ajouter_hooks(settings, commande)?;
    Ok(out)
}

/// `ajouter_hooks`.
#[pyfunction]
#[pyo3(name = "ajouter_hooks", signature = (settings, commande=None))]
fn ajouter_hooks(
    py: Python<'_>,
    settings: Bound<'_, PyAny>,
    commande: Option<String>,
) -> PyResult<Py<PyAny>> {
    ajouter_hooks_inner(settings, commande)
        .map_err(error)
        .and_then(|v| crate::pyconv::ToPy::to_py(&v, py))
}

fn retirer_hooks_inner(settings: Bound<'_, PyAny>) -> Result<serde_json::Value, String> {
    let settings: serde_json::Value = crate::pyconv::FromPy::from_py(&settings)?;
    let out = port::retirer_hooks(settings)?;
    Ok(out)
}

/// `retirer_hooks`.
#[pyfunction]
#[pyo3(name = "retirer_hooks", signature = (settings))]
fn retirer_hooks(py: Python<'_>, settings: Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
    retirer_hooks_inner(settings)
        .map_err(error)
        .and_then(|v| crate::pyconv::ToPy::to_py(&v, py))
}

fn transcript_autorise_inner(chemin: String, racine: Bound<'_, PyAny>) -> Result<bool, String> {
    let racine: serde_json::Value = crate::pyconv::FromPy::from_py(&racine)?;
    let out = port::transcript_autorise(chemin, racine)?;
    Ok(out)
}

/// `transcript_autorise`.
#[pyfunction]
#[pyo3(name = "transcript_autorise", signature = (chemin, racine))]
fn transcript_autorise(chemin: String, racine: Bound<'_, PyAny>) -> PyResult<bool> {
    transcript_autorise_inner(chemin, racine).map_err(error)
}

fn filtrer_inner(
    corps: Bound<'_, PyAny>,
    pid_brut: Option<String>,
    recu_a: f64,
    projets_claude: Bound<'_, PyAny>,
) -> Result<Option<serde_json::Value>, String> {
    let corps: serde_json::Value = crate::pyconv::FromPy::from_py(&corps)?;
    let projets_claude: serde_json::Value = crate::pyconv::FromPy::from_py(&projets_claude)?;
    let out = port::filtrer(corps, pid_brut, recu_a, projets_claude)?;
    Ok(out)
}

/// `filtrer`.
#[pyfunction]
#[pyo3(name = "filtrer", signature = (corps, pid_brut, recu_a, projets_claude))]
fn filtrer(
    py: Python<'_>,
    corps: Bound<'_, PyAny>,
    pid_brut: Option<String>,
    recu_a: f64,
    projets_claude: Bound<'_, PyAny>,
) -> PyResult<Py<PyAny>> {
    filtrer_inner(corps, pid_brut, recu_a, projets_claude)
        .map_err(error)
        .and_then(|v| crate::pyconv::ToPy::to_py(&v, py))
}

fn libelle_outil_inner(tool_name: Option<String>) -> Result<String, String> {
    let out = port::libelle_outil(tool_name)?;
    Ok(out)
}

/// `libelle_outil`.
#[pyfunction]
#[pyo3(name = "libelle_outil", signature = (tool_name))]
fn libelle_outil(tool_name: Option<String>) -> PyResult<String> {
    libelle_outil_inner(tool_name).map_err(error)
}

fn minuit_local_inner(maintenant: f64) -> Result<f64, String> {
    let out = port::minuit_local(maintenant)?;
    Ok(out)
}

/// `minuit_local`.
#[pyfunction]
#[pyo3(name = "minuit_local", signature = (maintenant))]
fn minuit_local(maintenant: f64) -> PyResult<f64> {
    minuit_local_inner(maintenant).map_err(error)
}

fn fichiers_de_session_inner(
    transcript: Bound<'_, PyAny>,
) -> Result<Vec<serde_json::Value>, String> {
    let transcript: serde_json::Value = crate::pyconv::FromPy::from_py(&transcript)?;
    let out = port::fichiers_de_session(transcript)?;
    Ok(out)
}

/// `fichiers_de_session`.
#[pyfunction]
#[pyo3(name = "fichiers_de_session", signature = (transcript))]
fn fichiers_de_session(py: Python<'_>, transcript: Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
    fichiers_de_session_inner(transcript)
        .map_err(error)
        .and_then(|v| crate::pyconv::ToPy::to_py(&v, py))
}

fn extraire_inner(obj: Bound<'_, PyAny>) -> Result<Option<serde_json::Value>, String> {
    let obj: serde_json::Value = crate::pyconv::FromPy::from_py(&obj)?;
    let out = port::extraire(obj)?;
    Ok(out)
}

/// `extraire`.
#[pyfunction]
#[pyo3(name = "extraire", signature = (obj))]
fn extraire(py: Python<'_>, obj: Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
    extraire_inner(obj)
        .map_err(error)
        .and_then(|v| crate::pyconv::ToPy::to_py(&v, py))
}

fn modele_de_inner(obj: Bound<'_, PyAny>) -> Result<Option<String>, String> {
    let obj: serde_json::Value = crate::pyconv::FromPy::from_py(&obj)?;
    let out = port::modele_de(obj)?;
    Ok(out)
}

/// `modele_de`.
#[pyfunction]
#[pyo3(name = "modele_de", signature = (obj))]
fn modele_de(obj: Bound<'_, PyAny>) -> PyResult<Option<String>> {
    modele_de_inner(obj).map_err(error)
}

#[pymodule]
#[pyo3(name = "_native")]
fn native_module(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_function(wrap_pyfunction!(formater_nombre, m)?)?;
    m.add_function(wrap_pyfunction!(nom_modele, m)?)?;
    m.add_function(wrap_pyfunction!(nom_projet, m)?)?;
    m.add_function(wrap_pyfunction!(est_vue_b, m)?)?;
    m.add_function(wrap_pyfunction!(charger, m)?)?;
    m.add_function(wrap_pyfunction!(ajouter_hooks, m)?)?;
    m.add_function(wrap_pyfunction!(retirer_hooks, m)?)?;
    m.add_function(wrap_pyfunction!(transcript_autorise, m)?)?;
    m.add_function(wrap_pyfunction!(filtrer, m)?)?;
    m.add_function(wrap_pyfunction!(libelle_outil, m)?)?;
    m.add_function(wrap_pyfunction!(minuit_local, m)?)?;
    m.add_function(wrap_pyfunction!(fichiers_de_session, m)?)?;
    m.add_function(wrap_pyfunction!(extraire, m)?)?;
    m.add_function(wrap_pyfunction!(modele_de, m)?)?;
    Ok(())
}

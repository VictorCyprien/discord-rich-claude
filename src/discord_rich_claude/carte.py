"""Construction de la carte Discord. Fonctions pures. Voir la spec, section 5."""
import functools
import re
from dataclasses import dataclass
from pathlib import Path

from .config import Config
from .sessions import Registre, activite_affichee

PROJET_PRIVE = "Projet privé"
LIMITE_DISCORD = 128
_MODELE = re.compile(r"^claude-(opus|sonnet|haiku|fable)-(\d+)(?:-(\d{1,2}))?(?:-\d{8})?$")


@dataclass(frozen=True)
class Carte:
    details: str
    state: str
    debut: int
    large_image: str = "logo"
    large_text: str = "Claude Code"


@dataclass(frozen=True)
class Stats:
    modele: str | None = None
    tokens_session: int = 0
    tokens_jour: int = 0


def formater_nombre(n: int) -> str:
    if n < 1_000:
        return str(n)
    if n < 1_000_000:
        return f"{n // 1_000} k"
    return f"{n / 1_000_000:.1f} M".replace(".", ",")


def nom_modele(model_id: str | None) -> str | None:
    if not model_id:
        return None
    m = _MODELE.match(model_id)
    if m is None:
        return model_id.removeprefix("claude-")
    famille, majeur, mineur = m.groups()
    version = f"{majeur}.{mineur}" if mineur else majeur
    return f"{famille.capitalize()} {version}"


@functools.lru_cache(maxsize=128)
def _resoudre(cwd: str) -> Path | None:
    # Mis en cache par dossier : appelé chaque seconde par la boucle carte, un
    # `resolve()` peut bloquer si le volume qui porte le projet est mal monté.
    try:
        return Path(cwd).resolve()
    except (OSError, RuntimeError):
        return None


def nom_projet(cwd: str | None, autorises: tuple[Path, ...]) -> str:
    # Comparaison sur chemins complets résolus : un homonyme ou un dossier dont le
    # nom commence pareil n'est jamais autorisé par erreur.
    if not cwd:
        return PROJET_PRIVE
    chemin = _resoudre(cwd)
    if chemin is None:
        return PROJET_PRIVE
    for autorise in autorises:
        if chemin == autorise or autorise in chemin.parents:
            return autorise.name
    return PROJET_PRIVE


def _tronquer(texte: str, limite: int = LIMITE_DISCORD) -> str:
    if len(texte) <= limite:
        return texte
    return texte[: limite - 1] + "…"


def est_vue_b(maintenant: float, rotation_s: float) -> bool:
    return int(maintenant // rotation_s) % 2 == 1


def construire_carte(
    registre: Registre, config: Config, stats: Stats, maintenant: float, vue_b: bool
) -> Carte | None:
    # Masquage (décision de Victor, 2026-09-18) : une session sans événement depuis
    # `masquer_apres_min` ne compte plus pour la carte, ni affichée ni comptée ni dans
    # le chrono. Calculé sur ce sous-ensemble, jamais sur `registre.session_affichee()`
    # / `registre.debut_plus_ancien()`, qui portent sur toutes les sessions vivantes.
    masquer_apres_s = config.masquer_apres_min * 60
    comptent = [
        s for s in registre.sessions.values() if maintenant - s.dernier_evenement < masquer_apres_s
    ]
    if not comptent:
        return None
    session = max(comptent, key=lambda s: s.dernier_evenement)
    activite = activite_affichee(session, maintenant, config.inactif_apres_min * 60)
    details = f"{activite} · {nom_projet(session.cwd, config.projets_autorises)}"
    if vue_b:
        n = len(comptent)
        pluriel = "s" if n > 1 else ""
        state = f"{n} session{pluriel} · {formater_nombre(stats.tokens_jour)} aujourd'hui"
    else:
        tokens = f"{formater_nombre(stats.tokens_session)} tokens"
        modele = nom_modele(stats.modele)
        state = f"{modele} · {tokens}" if modele else tokens
    # Discord refuse toute mise à jour dont `details` ou `state` dépasse 128
    # caractères (déconnexion, puis nouvel essai toutes les 15 s).
    return Carte(
        details=_tronquer(details),
        state=_tronquer(state),
        debut=int(min(s.debut for s in comptent)),
    )

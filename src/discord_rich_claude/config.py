"""Lecture de config.toml. Voir la spec, section 7.1.

Tout défaut de config retombe sur Config() : aucun projet autorisé, pas de
client_id, donc rien de montré (refus par défaut).
"""
import logging
import math
import tomllib
from dataclasses import dataclass
from pathlib import Path

log = logging.getLogger(__name__)


@dataclass(frozen=True)
class Config:
    client_id: str | None = None
    inactif_apres_min: float = 10
    rotation_s: float = 20
    projets_autorises: tuple[Path, ...] = ()


def charger(chemin: Path) -> Config:
    try:
        with chemin.open("rb") as f:
            brut = tomllib.load(f)
    except FileNotFoundError:
        log.warning("config absente : %s", chemin)
        return Config()
    except (OSError, tomllib.TOMLDecodeError, UnicodeDecodeError) as e:
        log.warning("config illisible, rien ne sera affiché : %s", e)
        return Config()
    try:
        return _valider(brut)
    except (TypeError, ValueError) as e:
        log.warning("config invalide, rien ne sera affiché : %s", e)
        return Config()


def _valider(brut: dict) -> Config:
    cid = brut.get("client_id")
    if cid is not None and (isinstance(cid, bool) or not isinstance(cid, (str, int))):
        raise TypeError("client_id doit être un texte")
    projets = brut.get("projets", {})
    if not isinstance(projets, dict):
        raise TypeError("[projets] doit être une table")
    autorises = projets.get("autorises", [])
    if not isinstance(autorises, list) or not all(isinstance(p, str) for p in autorises):
        raise TypeError("projets.autorises doit être une liste de chemins")
    chemins = []
    for p in autorises:
        try:
            chemin = Path(p).expanduser()
            if not chemin.is_absolute():
                raise ValueError(f"chemin relatif refusé : {p}")
            chemin = chemin.resolve()
        except RuntimeError:
            raise ValueError(f"chemin invalide refusé : {p}") from None
        chemins.append(chemin)
    return Config(
        client_id=str(cid) if cid not in (None, "") else None,
        inactif_apres_min=_positif(brut.get("inactif_apres_min", 10), "inactif_apres_min"),
        rotation_s=_positif(brut.get("rotation_s", 20), "rotation_s"),
        projets_autorises=tuple(chemins),
    )


def _positif(valeur: object, nom: str) -> float:
    if isinstance(valeur, bool) or not isinstance(valeur, (int, float)):
        raise TypeError(f"{nom} doit être un nombre")
    if not math.isfinite(valeur):
        raise ValueError(f"{nom} doit être fini")
    if valeur <= 0:
        raise ValueError(f"{nom} doit être positif")
    return valeur


def _mtime(chemin: Path) -> int | None:
    try:
        return chemin.stat().st_mtime_ns
    except OSError:
        return None


class ConfigSurveillee:
    """Recharge la config quand la date de modification du fichier change."""

    def __init__(self, chemin: Path) -> None:
        self.chemin = chemin
        self._mtime = _mtime(chemin)
        self.valeur = charger(chemin)

    def recharger_si_change(self) -> bool:
        mtime = _mtime(self.chemin)
        if mtime == self._mtime:
            return False
        self._mtime = mtime
        self.valeur = charger(self.chemin)
        return True

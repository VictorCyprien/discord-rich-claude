"""Lecture de config.toml. Voir la spec, section 7.1.

Tout défaut de config retombe sur Config() : aucun projet autorisé, pas de
client_id, donc rien de montré (refus par défaut).
"""
import logging
import math
import os
import tomllib
from dataclasses import dataclass
from pathlib import Path

log = logging.getLogger(__name__)


@dataclass(frozen=True)
class Config:
    client_id: str | None = None
    inactif_apres_min: float = 10
    masquer_apres_min: float = 60
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
    except (TypeError, ValueError, OverflowError) as e:
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
        inactif_apres_min=_borne(brut.get("inactif_apres_min", 10), "inactif_apres_min", 1, 1440),
        masquer_apres_min=_borne(brut.get("masquer_apres_min", 60), "masquer_apres_min", 1, 1440),
        rotation_s=_borne(brut.get("rotation_s", 20), "rotation_s", 5, 3600),
        projets_autorises=tuple(chemins),
    )


def _borne(valeur: object, nom: str, mini: float, maxi: float) -> float:
    if isinstance(valeur, bool) or not isinstance(valeur, (int, float)):
        raise TypeError(f"{nom} doit être un nombre")
    # Bornes avant `isfinite` : un entier énorme (des centaines de chiffres) fait
    # lever `OverflowError` à la conversion en float que `isfinite` ferait, mais
    # est de toute façon hors bornes — la comparaison directe entre entiers ne
    # convertit jamais en float et ne lève donc jamais.
    if not mini <= valeur <= maxi:
        raise ValueError(f"{nom} doit être compris entre {mini} et {maxi}")
    if not math.isfinite(valeur):
        raise ValueError(f"{nom} doit être fini")
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


CONFIG_PAR_DEFAUT = """\
# Configuration de discord-rich-claude.
# ID de l'application Discord : Developer Portal > General Information > Application ID.
client_id = ""
inactif_apres_min = 10
masquer_apres_min = 60
rotation_s = 20

[projets]
# Refusé par défaut : seuls ces dossiers (chemins complets) affichent leur nom.
autorises = []
"""


def creer_config_par_defaut(chemin: Path) -> bool:
    """Création exclusive, 0600 : la config contiendra des chemins de projets.

    O_EXCL avec O_CREAT échoue si `chemin` existe déjà, même comme lien
    symbolique pendant (vers une cible absente) — jamais suivi ni écrasé.
    """
    chemin.parent.mkdir(parents=True, exist_ok=True)
    try:
        fd = os.open(chemin, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    except FileExistsError:
        return False
    with os.fdopen(fd, "w", encoding="utf-8") as f:
        f.write(CONFIG_PAR_DEFAUT)
    return True

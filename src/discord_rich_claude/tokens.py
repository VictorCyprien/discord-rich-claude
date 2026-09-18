"""Tokens et modèle lus dans les transcripts JSONL de Claude Code. Voir la spec, section 6.

Les transcripts répètent une même réponse avec des compteurs différents (écriture
en flux) : on garde une entrée par (message.id, requestId), avec le maximum de
chaque champ. Garder la première occurrence sous-compte.
"""
import json
import logging
from collections.abc import Iterable
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path

log = logging.getLogger(__name__)

# Les lectures du cache sont exclues : elles grossissent avec la longueur du
# contexte, pas avec le travail fait.
CHAMPS_COMPTES = ("input_tokens", "cache_creation_input_tokens", "output_tokens")

Cle = tuple[str, str]


@dataclass
class Entree:
    horodatage: float | None
    tokens: dict[str, int]


def minuit_local(maintenant: float) -> float:
    d = datetime.fromtimestamp(maintenant).astimezone()
    return d.replace(hour=0, minute=0, second=0, microsecond=0).timestamp()


def fichiers_de_session(transcript: Path) -> list[Path]:
    sous_agents = transcript.with_suffix("") / "subagents"
    return [transcript, *sorted(sous_agents.glob("*.jsonl"))]


def _horodatage(brut: object) -> float | None:
    if not isinstance(brut, str):
        return None
    try:
        return datetime.fromisoformat(brut).timestamp()
    except ValueError:
        return None


def extraire(obj: dict) -> tuple[Cle, float | None, dict[str, int]] | None:
    msg = obj.get("message")
    if not isinstance(msg, dict) or not isinstance(msg.get("usage"), dict):
        return None
    ident = msg.get("id") or obj.get("uuid")
    if not ident:
        return None
    usage = msg["usage"]
    try:
        tokens = {k: int(usage.get(k) or 0) for k in CHAMPS_COMPTES}
    except (TypeError, ValueError):
        return None
    cle = (str(ident), str(obj.get("requestId") or ""))
    return cle, _horodatage(obj.get("timestamp")), tokens


def modele_de(obj: dict) -> str | None:
    msg = obj.get("message")
    if obj.get("type") != "assistant" or not isinstance(msg, dict):
        return None
    modele = msg.get("model")
    if isinstance(modele, str) and modele and modele != "<synthetic>":
        return modele
    return None


class Fichier:
    """Un transcript lu par morceaux : seules les nouvelles lignes complètes."""

    def __init__(self, chemin: Path) -> None:
        self.chemin = chemin
        self.decalage = 0
        self.entrees: dict[Cle, Entree] = {}
        self.modele: str | None = None

    def rafraichir(self) -> None:
        for obj in self._nouvelles_lignes():
            extrait = extraire(obj)
            if extrait is not None:
                cle, horodatage, tokens = extrait
                entree = self.entrees.get(cle)
                if entree is None:
                    self.entrees[cle] = Entree(horodatage, tokens)
                else:
                    entree.tokens = {k: max(v, entree.tokens.get(k, 0)) for k, v in tokens.items()}
            modele = modele_de(obj)
            if modele is not None:
                self.modele = modele

    def _nouvelles_lignes(self) -> list[dict]:
        try:
            if self.chemin.stat().st_size < self.decalage:
                # Fichier réécrit plus court : on repart de zéro.
                self.decalage, self.entrees, self.modele = 0, {}, None
            with self.chemin.open("rb") as f:
                f.seek(self.decalage)
                bloc = f.read()
        except FileNotFoundError:
            return []
        fin = bloc.rfind(b"\n")
        if fin < 0:
            return []
        self.decalage += fin + 1
        objets = []
        for brute in bloc[:fin].split(b"\n"):
            if not brute.strip():
                continue
            try:
                obj = json.loads(brute)
            except ValueError:
                log.warning("ligne illisible ignorée dans %s", self.chemin.name)
                continue
            if isinstance(obj, dict):
                objets.append(obj)
        return objets


def _total(fichiers: Iterable[Fichier], depuis: float | None = None) -> int:
    fusion: dict[Cle, dict[str, int]] = {}
    for fichier in fichiers:
        for cle, entree in fichier.entrees.items():
            if depuis is not None and (entree.horodatage is None or entree.horodatage < depuis):
                continue
            courant = fusion.setdefault(cle, {})
            for k, v in entree.tokens.items():
                courant[k] = max(courant.get(k, 0), v)
    return sum(sum(t.values()) for t in fusion.values())


class SuiviTokens:
    def __init__(self, racine: Path) -> None:
        self.racine = racine
        self._fichiers: dict[Path, Fichier] = {}

    @property
    def fichiers_suivis(self) -> frozenset[Path]:
        return frozenset(self._fichiers)

    def rafraichir(self, transcripts: Iterable[Path], minuit: float) -> None:
        voulus: set[Path] = set()
        for transcript in transcripts:
            voulus.update(fichiers_de_session(transcript))
        for chemin in self.racine.glob("**/*.jsonl"):
            try:
                if chemin.stat().st_mtime >= minuit:
                    voulus.add(chemin)
            except FileNotFoundError:
                pass
        for chemin in list(self._fichiers):
            if chemin not in voulus:
                del self._fichiers[chemin]
        for chemin in voulus:
            self._fichiers.setdefault(chemin, Fichier(chemin)).rafraichir()

    def tokens_session(self, transcript: Path) -> int:
        return _total(
            self._fichiers[p] for p in fichiers_de_session(transcript) if p in self._fichiers
        )

    def tokens_jour(self, minuit: float) -> int:
        return _total(self._fichiers.values(), depuis=minuit)

    def modele(self, transcript: Path) -> str | None:
        fichier = self._fichiers.get(transcript)
        return fichier.modele if fichier else None

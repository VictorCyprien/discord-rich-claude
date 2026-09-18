"""Réception des hooks sur un socket Unix. Voir la spec, sections 3, 7.3 et 7.5.

HTTP minimal codé à la main : une route d'écriture, une de lecture, rien d'autre.
Le JSON d'un hook contient le prompt et les entrées d'outil : filtrer() n'en
garde qu'une liste fermée de champs, le reste n'est jamais conservé.
"""
import asyncio
import functools
import json
import logging
import os
import re
import socket
import time
from collections.abc import Callable
from contextlib import suppress
from pathlib import Path

from .sessions import Evenement

log = logging.getLogger(__name__)

TAILLE_MAX = 16 * 1024 * 1024  # un PreToolUse sur Write transporte tout le fichier
DELAI_S = 2.0
CHAMPS_LUS = ("session_id", "cwd", "hook_event_name", "tool_name", "agent_id", "transcript_path")
EVENEMENTS = frozenset(
    {
        "SessionStart", "UserPromptSubmit", "PreToolUse", "PermissionRequest",
        "Stop", "SubagentStart", "SubagentStop", "SessionEnd",
    }
)
SESSION_ID = re.compile(r"^[A-Za-z0-9_-]{1,128}$")
RAISONS = {
    200: "OK", 204: "No Content", 404: "Not Found", 411: "Length Required",
    413: "Content Too Large", 500: "Internal Server Error",
}


class DejaLance(RuntimeError):
    """Un autre daemon écoute déjà sur le socket."""


def transcript_autorise(chemin: str, racine: Path) -> bool:
    p = Path(chemin)
    if not p.is_absolute() or p.suffix != ".jsonl":
        return False
    return racine.resolve() in p.resolve().parents


def _pid(brut: str | None) -> int | None:
    if brut and brut.isascii() and brut.isdigit():
        pid = int(brut)
        return pid if pid > 1 else None
    return None


def filtrer(corps: bytes, pid_brut: str | None, recu_a: float, projets_claude: Path) -> Evenement | None:
    try:
        brut = json.loads(corps)
    except ValueError:
        return None
    if not isinstance(brut, dict):
        return None
    champs = {k: brut.get(k) for k in CHAMPS_LUS}
    del brut
    if any(v is not None and not isinstance(v, str) for v in champs.values()):
        return None
    nom, session_id = champs["hook_event_name"], champs["session_id"]
    if nom not in EVENEMENTS or not session_id or not SESSION_ID.match(session_id):
        return None
    transcript = champs["transcript_path"]
    if transcript is not None and not transcript_autorise(transcript, projets_claude):
        transcript = None
    return Evenement(
        nom=nom,
        session_id=session_id,
        recu_a=recu_a,
        cwd=champs["cwd"],
        tool_name=champs["tool_name"],
        agent_id=champs["agent_id"],
        transcript_path=transcript,
        pid=_pid(pid_brut),
    )


async def _repondre(ecrivain: asyncio.StreamWriter, code: int, corps: bytes = b"") -> None:
    tete = f"HTTP/1.1 {code} {RAISONS[code]}\r\nContent-Length: {len(corps)}\r\nConnection: close\r\n"
    if corps:
        tete += "Content-Type: application/json\r\n"
    ecrivain.write(tete.encode() + b"\r\n" + corps)
    await ecrivain.drain()


async def _traiter(
    lecteur: asyncio.StreamReader,
    ecrivain: asyncio.StreamWriter,
    *,
    sur_evenement: Callable[[Evenement], None],
    etat: Callable[[], dict],
    projets_claude: Path,
) -> None:
    try:
        tete = await asyncio.wait_for(lecteur.readuntil(b"\r\n\r\n"), DELAI_S)
        lignes = tete.decode("latin-1").split("\r\n")
        methode, chemin, _ = lignes[0].split(" ", 2)
        entetes = {}
        for ligne in lignes[1:]:
            if ":" in ligne:
                cle, valeur = ligne.split(":", 1)
                entetes[cle.strip().lower()] = valeur.strip()
        if methode == "GET" and chemin == "/etat":
            await _repondre(ecrivain, 200, json.dumps(etat()).encode())
            return
        if methode != "POST" or chemin != "/event":
            await _repondre(ecrivain, 404)
            return
        if "content-length" not in entetes:
            await _repondre(ecrivain, 411)
            return
        longueur = int(entetes["content-length"])
        if not 0 <= longueur <= TAILLE_MAX:
            await _repondre(ecrivain, 413)
            return
        if entetes.get("expect", "").lower() == "100-continue":
            ecrivain.write(b"HTTP/1.1 100 Continue\r\n\r\n")
            await ecrivain.drain()
        corps = await asyncio.wait_for(lecteur.readexactly(longueur), DELAI_S)
        ev = filtrer(corps, entetes.get("x-claude-pid"), time.time(), projets_claude)
        del corps
        if ev is not None:
            try:
                sur_evenement(ev)
            except Exception:
                log.exception("événement %s en erreur", ev.nom)
                await _repondre(ecrivain, 500)
                return
        await _repondre(ecrivain, 204)
    except (asyncio.IncompleteReadError, asyncio.LimitOverrunError, TimeoutError, ValueError, ConnectionError) as e:
        log.debug("requête rejetée : %s", type(e).__name__)
    finally:
        ecrivain.close()
        with suppress(Exception):
            await ecrivain.wait_closed()


async def demarrer_serveur(
    chemin_socket: Path,
    sur_evenement: Callable[[Evenement], None],
    etat: Callable[[], dict],
    projets_claude: Path,
) -> asyncio.Server:
    dossier = chemin_socket.parent
    dossier.mkdir(parents=True, exist_ok=True)
    os.chmod(dossier, 0o700)
    if chemin_socket.exists():
        try:
            _, ecrivain = await asyncio.open_unix_connection(str(chemin_socket))
        except OSError:
            chemin_socket.unlink()  # socket orphelin d'un daemon mort
        else:
            ecrivain.close()
            raise DejaLance(str(chemin_socket))
    gestionnaire = functools.partial(
        _traiter, sur_evenement=sur_evenement, etat=etat, projets_claude=projets_claude
    )
    ancien = os.umask(0o177)
    try:
        serveur = await asyncio.start_unix_server(gestionnaire, path=str(chemin_socket))
    finally:
        os.umask(ancien)
    os.chmod(chemin_socket, 0o600)
    return serveur


def interroger_etat(chemin_socket: Path, delai_s: float = 1.0) -> dict | None:
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:
            s.settimeout(delai_s)
            s.connect(str(chemin_socket))
            s.sendall(b"GET /etat HTTP/1.1\r\nHost: localhost\r\n\r\n")
            reponse = b""
            while morceau := s.recv(4096):
                reponse += morceau
    except OSError:
        return None
    tete, _, corps = reponse.partition(b"\r\n\r\n")
    if not tete.startswith(b"HTTP/1.1 200"):
        return None
    try:
        return json.loads(corps)
    except ValueError:
        return None

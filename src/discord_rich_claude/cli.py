"""Ligne de commande. Voir la spec, section 8."""
import argparse
import asyncio
import logging
import signal
import sys

from . import chemins
from .config import ConfigSurveillee
from .daemon import Daemon, configurer_logs
from .presence import ClientPypresence, Presence
from .serveur import DejaLance
from .tokens import SuiviTokens

log = logging.getLogger(__name__)


def lancer() -> int:
    configurer_logs(chemins.DOSSIER_LOGS)

    async def principal() -> None:
        arret = asyncio.Event()
        boucle = asyncio.get_running_loop()
        for sig in (signal.SIGTERM, signal.SIGINT):
            boucle.add_signal_handler(sig, arret.set)
        daemon = Daemon(
            ConfigSurveillee(chemins.CONFIG),
            Presence(ClientPypresence),
            SuiviTokens(chemins.PROJETS_CLAUDE),
            chemins.PROJETS_CLAUDE,
        )
        await daemon.executer(chemins.SOCKET, arret)

    try:
        asyncio.run(principal())
    except DejaLance:
        log.error("un daemon tourne déjà sur %s", chemins.SOCKET)
        print(f"discord-rich-claude : un daemon tourne déjà sur {chemins.SOCKET}", file=sys.stderr)
        return 1
    except OSError as e:
        # PermissionError (dossier d'état invalide), ou toute autre erreur système au
        # démarrage : une ligne dans le log et sur stderr, jamais la trace complète —
        # sous le LaunchAgent (KeepAlive), une trace deviendrait une boucle de crash
        # écrite en continu dans un fichier non tourné.
        log.error("démarrage impossible (%s), socket %s", type(e).__name__, chemins.SOCKET)
        print(
            f"discord-rich-claude : démarrage impossible ({type(e).__name__}), "
            f"socket {chemins.SOCKET}",
            file=sys.stderr,
        )
        return 1
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="discord-rich-claude", description="Discord Rich Presence pour Claude Code"
    )
    sous = parser.add_subparsers(dest="commande", required=True)
    sous.add_parser("run", help="lance le daemon (utilisé par le LaunchAgent)")
    args = parser.parse_args(argv)
    return {"run": lancer}[args.commande]()

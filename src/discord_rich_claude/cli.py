"""Ligne de commande. Voir la spec, section 8."""
import argparse
import asyncio
import logging
import os
import shutil
import signal
import subprocess
import sys
from pathlib import Path

from . import chemins
from .config import ConfigSurveillee, creer_config_par_defaut
from .daemon import Daemon, configurer_logs
from .installation import (
    ajouter_hooks,
    desinstaller_launch_agent,
    generer_plist,
    installer_launch_agent,
    modifier_settings,
    retirer_hooks,
)
from .presence import ClientPypresence, Presence
from .serveur import DejaLance, interroger_etat
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


def _confirmer(diff: str) -> bool:
    print(diff)
    try:
        reponse = input(f"Appliquer ces changements à {chemins.SETTINGS_CLAUDE} ? [o/N] ")
    except (EOFError, KeyboardInterrupt):
        print()
        return False
    return reponse.strip().lower() == "o"


def _executable_absolu() -> str | None:
    """Chemin absolu du binaire à lancer par le LaunchAgent.

    Préfère le binaire en cours d'exécution (`sys.argv[0]`) : sous `uv run`,
    `which()` peut trouver un autre exécutable dans le PATH que celui réellement
    lancé (ex. un checkout de dev sur un volume externe non monté au login).
    launchd ne peut pas démarrer un chemin relatif : on refuse plutôt que deviner.
    """
    argv0 = Path(sys.argv[0])
    if argv0.is_absolute() and argv0.is_file():
        return str(argv0)
    executable = shutil.which("discord-rich-claude")
    if executable and Path(executable).is_absolute():
        return executable
    return None


def installer() -> int:
    if os.geteuid() == 0:
        print("ne pas lancer avec sudo : relance sans sudo", file=sys.stderr)
        return 1
    print("1/3 Hooks Claude Code")
    try:
        resultat = modifier_settings(chemins.SETTINGS_CLAUDE, ajouter_hooks, _confirmer)
    except ValueError as e:
        print(f"settings.json non modifié : {e}", file=sys.stderr)
        return 1
    if resultat == "refusé":
        print("  installation annulée, rien n'a été modifié")
        return 1
    print(f"  {resultat}")
    print("2/3 Configuration")
    try:
        if creer_config_par_defaut(chemins.CONFIG):
            print(f"  créée : {chemins.CONFIG}. Renseigne client_id et projets.autorises.")
        else:
            print(f"  déjà présente : {chemins.CONFIG}")
    except OSError as e:
        print(f"  {type(e).__name__} : {chemins.CONFIG}", file=sys.stderr)
        return 1
    print("3/3 LaunchAgent")
    executable = _executable_absolu()
    if executable is None:
        print(
            "exécutable introuvable ou non absolu : installe avec `uv tool install .` "
            "et relance `discord-rich-claude install`",
            file=sys.stderr,
        )
        return 1
    print(f"  exécutable : {executable}")
    try:
        chemins.DOSSIER_LOGS.mkdir(parents=True, exist_ok=True)
    except OSError as e:
        print(f"  {type(e).__name__} : {chemins.DOSSIER_LOGS}", file=sys.stderr)
        return 1
    try:
        installer_launch_agent(chemins.LAUNCH_AGENT, generer_plist(executable, chemins.DOSSIER_LOGS))
    except OSError as e:
        print(f"  {type(e).__name__} : {chemins.LAUNCH_AGENT}", file=sys.stderr)
        return 1
    except subprocess.CalledProcessError as e:
        detail = (e.stderr or b"").decode(errors="replace").strip()
        print(
            f"  launchctl a échoué ({e.returncode}) : {detail} — relance `discord-rich-claude install`",
            file=sys.stderr,
        )
        return 1
    print("  LaunchAgent chargé")
    return 0


def desinstaller() -> int:
    if os.geteuid() == 0:
        print("ne pas lancer avec sudo : relance sans sudo", file=sys.stderr)
        return 1
    try:
        resultat = modifier_settings(chemins.SETTINGS_CLAUDE, retirer_hooks, _confirmer)
    except ValueError as e:
        print(f"settings.json non modifié : {e}", file=sys.stderr)
        return 1
    if resultat == "refusé":
        print("désinstallation annulée")
        return 1
    desinstaller_launch_agent(chemins.LAUNCH_AGENT)
    print(f"Désinstallé. Config conservée : {chemins.CONFIG}")
    return 0


def statut() -> int:
    etat = interroger_etat(chemins.SOCKET)
    if etat is None:
        print("daemon : injoignable")
        return 1
    print("daemon : actif")
    print(f"discord : {'connecté' if etat.get('discord') else 'non connecté'}")
    print(f"sessions : {etat.get('sessions', 0)}")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="discord-rich-claude", description="Discord Rich Presence pour Claude Code"
    )
    sous = parser.add_subparsers(dest="commande", required=True)
    sous.add_parser("run", help="lance le daemon (utilisé par le LaunchAgent)")
    sous.add_parser("install", help="ajoute les hooks, crée la config et le LaunchAgent")
    sous.add_parser("uninstall", help="retire nos hooks et le LaunchAgent, garde la config")
    sous.add_parser("status", help="état du daemon")
    args = parser.parse_args(argv)
    commandes = {"run": lancer, "install": installer, "uninstall": desinstaller, "status": statut}
    return commandes[args.commande]()

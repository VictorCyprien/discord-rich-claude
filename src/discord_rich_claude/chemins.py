"""Emplacements sur disque et commande de hook. Voir la spec, sections 7.5 et 8."""
from pathlib import Path

HOME = Path.home()
DOSSIER_ETAT = HOME / ".local" / "state" / "discord-rich-claude"
SOCKET = DOSSIER_ETAT / "daemon.sock"
CONFIG = HOME / ".config" / "discord-rich-claude" / "config.toml"
DOSSIER_LOGS = HOME / "Library" / "Logs" / "discord-rich-claude"
PROJETS_CLAUDE = HOME / ".claude" / "projects"
SETTINGS_CLAUDE = HOME / ".claude" / "settings.json"
LABEL = "local.discord-rich-claude"
LAUNCH_AGENT = HOME / "Library" / "LaunchAgents" / f"{LABEL}.plist"

# $HOME et $PPID sont développés par le shell du hook : $PPID y désigne le
# processus Claude Code, que le daemon surveille pour purger les sessions mortes.
COMMANDE_HOOK = (
    'curl -s --max-time 1 --unix-socket "$HOME/.local/state/discord-rich-claude/daemon.sock" '
    '-H "X-Claude-Pid: $PPID" --data-binary @- http://localhost/event >/dev/null 2>&1 || true'
)

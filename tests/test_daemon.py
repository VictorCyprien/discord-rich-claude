import asyncio
import json
import os
import time
from datetime import datetime

from discord_rich_claude.chemins import COMMANDE_HOOK
from discord_rich_claude.config import ConfigSurveillee
from discord_rich_claude.daemon import Daemon
from discord_rich_claude.presence import Presence
from discord_rich_claude.tokens import SuiviTokens


class FauxClient:
    def __init__(self, journal):
        self.journal = journal

    async def connecter(self):
        self.journal.append("connecter")

    async def publier(self, carte):
        self.journal.append(carte)

    async def effacer(self):
        self.journal.append("effacer")

    async def fermer(self):
        self.journal.append("fermer")


async def attendre(condition, delai=5.0):
    fin = time.monotonic() + delai
    while not condition():
        if time.monotonic() > fin:
            raise AssertionError("condition jamais atteinte")
        await asyncio.sleep(0.05)


async def envoyer_hook(home, payload):
    """Lance la commande de hook exacte, comme Claude Code le ferait."""
    proc = await asyncio.create_subprocess_shell(
        COMMANDE_HOOK,
        stdin=asyncio.subprocess.PIPE,
        env={"HOME": str(home), "PATH": os.environ["PATH"]},
    )
    await proc.communicate(json.dumps(payload).encode())
    assert proc.returncode == 0


async def test_bout_en_bout(dossier_court, tmp_path):
    home = dossier_court
    socket_ = home / ".local" / "state" / "discord-rich-claude" / "daemon.sock"
    projets_claude = tmp_path / "projects"
    transcript = projets_claude / "p" / "s1.jsonl"
    transcript.parent.mkdir(parents=True)
    transcript.write_text(
        json.dumps(
            {
                "type": "assistant",
                "requestId": "r1",
                "timestamp": datetime.now().astimezone().isoformat(),
                "message": {
                    "id": "m1",
                    "model": "claude-opus-5",
                    "usage": {
                        "input_tokens": 10,
                        "output_tokens": 5,
                        "cache_creation_input_tokens": 100,
                        "cache_read_input_tokens": 9999,
                    },
                },
            }
        )
        + "\n",
        encoding="utf-8",
    )
    projet = tmp_path / "MonProjet"
    projet.mkdir()
    config = tmp_path / "config.toml"
    config.write_text(f'client_id = "42"\n[projets]\nautorises = ["{projet}"]\n', encoding="utf-8")

    journal = []
    presence = Presence(lambda cid: FauxClient(journal), intervalle_min_s=0, reessai_s=0)
    daemon = Daemon(ConfigSurveillee(config), presence, SuiviTokens(projets_claude), projets_claude)
    arret = asyncio.Event()
    tache = asyncio.create_task(daemon.executer(socket_, arret))
    await attendre(socket_.exists)

    await envoyer_hook(
        home,
        {
            "hook_event_name": "SessionStart",
            "session_id": "s1",
            "cwd": str(projet),
            "transcript_path": str(transcript),
        },
    )
    await envoyer_hook(
        home,
        {
            "hook_event_name": "PreToolUse",
            "session_id": "s1",
            "tool_name": "Edit",
            "tool_input": {"content": "SECRET-CONTENU"},
        },
    )
    await daemon.controler()

    session = daemon.registre.sessions["s1"]
    assert session.pid == os.getpid()  # $PPID du shell de hook = ce processus
    assert "SECRET" not in repr(session)
    assert daemon.etat()["sessions"] == 1

    def carte_attendue():
        cartes = [c for c in journal if not isinstance(c, str)]
        return any(
            c.details == "Écrit du code · MonProjet"
            and c.state in ("Opus 5 · 115 tokens", "1 session · 115 aujourd'hui")
            for c in cartes
        )

    await attendre(carte_attendue)

    await envoyer_hook(home, {"hook_event_name": "SessionEnd", "session_id": "s1"})
    await attendre(lambda: "effacer" in journal)

    arret.set()
    await asyncio.wait_for(tache, 5)
    assert not socket_.exists()

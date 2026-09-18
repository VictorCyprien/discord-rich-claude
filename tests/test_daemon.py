import asyncio
import json
import logging
import os
import time
from contextlib import suppress
from datetime import datetime

from discord_rich_claude.chemins import COMMANDE_HOOK
from discord_rich_claude.config import ConfigSurveillee
from discord_rich_claude.daemon import Daemon, configurer_logs
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


async def test_bout_en_bout_rien_de_prive_ne_fuit(dossier_court, tmp_path, caplog):
    """F8 : un marqueur porté par `prompt`, `tool_input` et `last_assistant_message`
    (jetés à la réception, jamais gardés, cf. spec 7.3) et le nom d'un dossier non
    autorisé ne doivent jamais apparaître, ni dans un enregistrement de log (message
    ou arguments, tous les logueurs, au niveau DEBUG), ni dans un champ de la carte
    publiée à Discord ; la carte d'un projet non autorisé se termine par « Projet
    privé »."""
    home = dossier_court
    socket_ = home / ".local" / "state" / "discord-rich-claude" / "daemon.sock"
    marqueur = "SECRET-MARQUEUR-9F3C"
    nom_dossier_prive = "ProjetClientConfidentielXYZ"
    dossier_prive = tmp_path / nom_dossier_prive
    dossier_prive.mkdir()
    config = tmp_path / "config.toml"
    config.write_text('client_id = "42"\n[projets]\nautorises = []\n', encoding="utf-8")
    projets_claude = tmp_path / "projects"
    projets_claude.mkdir()

    journal = []
    presence = Presence(lambda cid: FauxClient(journal), intervalle_min_s=0, reessai_s=0)
    daemon = Daemon(ConfigSurveillee(config), presence, SuiviTokens(projets_claude), projets_claude)
    arret = asyncio.Event()

    with caplog.at_level(logging.DEBUG):
        tache = asyncio.create_task(daemon.executer(socket_, arret))
        await attendre(socket_.exists)

        await envoyer_hook(
            home,
            {
                "hook_event_name": "UserPromptSubmit",
                "session_id": "s1",
                "cwd": str(dossier_prive),
                "prompt": marqueur,
            },
        )
        await envoyer_hook(
            home,
            {
                "hook_event_name": "PreToolUse",
                "session_id": "s1",
                "tool_name": "Edit",
                "tool_input": {"content": marqueur},
            },
        )
        await envoyer_hook(
            home,
            {
                "hook_event_name": "Stop",
                "session_id": "s1",
                "last_assistant_message": marqueur,
            },
        )
        await daemon.controler()

        def carte_publiee():
            return any(not isinstance(c, str) for c in journal)

        await attendre(carte_publiee)

        arret.set()
        await asyncio.wait_for(tache, 5)

    for record in caplog.records:
        message = record.getMessage()
        assert marqueur not in message
        assert nom_dossier_prive not in message
        for arg in record.args if isinstance(record.args, tuple) else ():
            assert marqueur not in str(arg)
            assert nom_dossier_prive not in str(arg)

    cartes = [c for c in journal if not isinstance(c, str)]
    assert cartes
    for c in cartes:
        assert marqueur not in c.details
        assert marqueur not in c.state
        assert nom_dossier_prive not in c.details
        assert nom_dossier_prive not in c.state
        assert c.details.endswith("Projet privé")


async def test_client_id_absent_logge_une_erreur_au_demarrage_et_apres_rechargement(
    tmp_path, caplog
):
    """Spec 7.1 : client_id absent → pas de connexion à Discord, erreur dans le log.
    Signalé une fois au démarrage, jamais à chaque tour, puis à nouveau seulement si
    un rechargement confirme qu'il manque toujours."""
    config = tmp_path / "config.toml"
    config.write_text("", encoding="utf-8")
    projets_claude = tmp_path / "projects"
    projets_claude.mkdir()
    journal = []
    presence = Presence(lambda cid: FauxClient(journal), intervalle_min_s=0, reessai_s=0)
    daemon = Daemon(ConfigSurveillee(config), presence, SuiviTokens(projets_claude), projets_claude)

    with caplog.at_level(logging.ERROR, logger="discord_rich_claude.daemon"):
        await daemon.controler()
        await daemon.controler()
        await daemon.controler()

    erreurs = [r for r in caplog.records if r.levelno == logging.ERROR]
    assert len(erreurs) == 1
    assert "client_id absent" in erreurs[0].getMessage()

    stat = config.stat()
    config.write_text("inactif_apres_min = 5\n", encoding="utf-8")
    os.utime(config, ns=(stat.st_atime_ns, stat.st_mtime_ns + 1_000_000_000))

    with caplog.at_level(logging.ERROR, logger="discord_rich_claude.daemon"):
        await daemon.controler()

    erreurs = [r for r in caplog.records if r.levelno == logging.ERROR]
    assert len(erreurs) == 2


async def test_boucle_ne_repete_pas_la_meme_erreur_et_signale_le_retablissement(caplog):
    """Une erreur qui persiste dans une boucle n'écrit qu'une ligne ERROR (le type
    seul), la trace complète part en DEBUG ; un changement de type ou un
    rétablissement produit une nouvelle ligne. Un chemin porté par l'exception (comme
    en produirait carte.py ou tokens.py sur une entrée hostile, cf. spec 7.5) ne doit
    jamais apparaître en dehors du niveau DEBUG."""
    marqueur = "/chemin/tres/secret/fourni/par/un/hook"
    compteur = {"n": 0}

    async def action():
        compteur["n"] += 1
        n = compteur["n"]
        if n <= 5:
            raise RuntimeError(f"echec sur {marqueur}")
        if n == 6:
            return
        raise ValueError(f"autre echec sur {marqueur}")

    tache = asyncio.create_task(Daemon._boucle(action, 0, "test"))
    with caplog.at_level(logging.DEBUG, logger="discord_rich_claude.daemon"):
        await attendre(lambda: compteur["n"] >= 7)
        await asyncio.sleep(0.05)
    tache.cancel()
    with suppress(asyncio.CancelledError):
        await tache

    erreurs = [r for r in caplog.records if r.levelno == logging.ERROR]
    infos = [r for r in caplog.records if r.levelno == logging.INFO]
    debogages = [r for r in caplog.records if r.levelno == logging.DEBUG]

    assert len(erreurs) == 2
    assert "RuntimeError" in erreurs[0].getMessage()
    assert "ValueError" in erreurs[1].getMessage()
    assert len(infos) == 1
    assert "rétabli" in infos[0].getMessage()

    for r in erreurs + infos:
        assert marqueur not in r.getMessage()
        assert not r.exc_info

    assert any(r.exc_info and marqueur in str(r.exc_info[1]) for r in debogages)


def test_drc_log_invalide_replie_sur_info_avec_avertissement(monkeypatch, tmp_path, caplog):
    """Une valeur DRC_LOG invalide ne doit jamais faire planter le démarrage (G3) :
    repli sur INFO, un avertissement dans le log."""
    monkeypatch.setenv("DRC_LOG", "NIMPORTE")
    racine = logging.getLogger()
    gestionnaires_avant = set(racine.handlers)
    niveau_avant = racine.level
    try:
        with caplog.at_level(logging.WARNING, logger="discord_rich_claude.daemon"):
            configurer_logs(tmp_path)
        assert racine.getEffectiveLevel() == logging.INFO
        avertissements = [r for r in caplog.records if r.levelno == logging.WARNING]
        assert len(avertissements) == 1
        assert "NIMPORTE" in avertissements[0].getMessage()
    finally:
        for h in list(racine.handlers):
            if h not in gestionnaires_avant:
                racine.removeHandler(h)
                h.close()
        racine.setLevel(niveau_avant)

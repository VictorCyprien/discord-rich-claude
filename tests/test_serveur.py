import asyncio
import json
import stat

import pytest

from discord_rich_claude.serveur import (
    DejaLance,
    demarrer_serveur,
    filtrer,
    interroger_etat,
    transcript_autorise,
)


def corps(**champs):
    return json.dumps(champs).encode()


def test_filtrer_ne_garde_que_la_liste_fermee(tmp_path):
    ev = filtrer(
        corps(
            hook_event_name="PreToolUse",
            session_id="abc-123",
            cwd="/p",
            tool_name="Write",
            tool_input={"file_path": "/secret/fichier.py", "content": "SECRET-CONTENU"},
            prompt="SECRET-PROMPT",
            last_assistant_message="SECRET-REPONSE",
        ),
        "4242",
        10.0,
        tmp_path,
    )
    assert (ev.nom, ev.session_id, ev.cwd, ev.tool_name, ev.pid, ev.recu_a) == (
        "PreToolUse", "abc-123", "/p", "Write", 4242, 10.0,
    )
    assert "SECRET" not in repr(ev)


@pytest.mark.parametrize(
    "brut",
    [
        b"{pas du json",
        b"\xff\xfe",
        b"[1, 2]",
        corps(hook_event_name="Notification", session_id="s1"),
        corps(hook_event_name="Stop"),
        corps(hook_event_name="Stop", session_id=""),
        corps(hook_event_name="Stop", session_id="../../etc"),
        corps(hook_event_name="Stop", session_id="s1", tool_name=3),
    ],
)
def test_filtrer_refuse(brut, tmp_path):
    assert filtrer(brut, None, 0.0, tmp_path) is None


@pytest.mark.parametrize(
    "pid_brut,attendu", [("123", 123), (None, None), ("abc", None), ("1", None), ("0", None), ("１２", None)]
)
def test_filtrer_pid(pid_brut, attendu, tmp_path):
    assert filtrer(corps(hook_event_name="Stop", session_id="s1"), pid_brut, 0.0, tmp_path).pid == attendu


def test_transcript_autorise(tmp_path):
    racine = tmp_path / "projects"
    (racine / "p").mkdir(parents=True)
    assert transcript_autorise(str(racine / "p" / "s1.jsonl"), racine)
    assert not transcript_autorise(str(racine / "p" / "s1.txt"), racine)
    assert not transcript_autorise(str(tmp_path / "ailleurs.jsonl"), racine)
    assert not transcript_autorise(str(racine / ".." / "ailleurs.jsonl"), racine)
    assert not transcript_autorise("relatif/s1.jsonl", racine)


def test_filtrer_efface_un_transcript_hors_racine(tmp_path):
    ev = filtrer(
        corps(hook_event_name="Stop", session_id="s1", transcript_path="/etc/passwd.jsonl"),
        None,
        0.0,
        tmp_path,
    )
    assert ev.transcript_path is None


async def requete(socket_, brute: bytes) -> bytes:
    lecteur, ecrivain = await asyncio.open_unix_connection(str(socket_))
    ecrivain.write(brute)
    await ecrivain.drain()
    reponse = await lecteur.read()
    ecrivain.close()
    return reponse


def post(corps_: bytes, entetes: str = "") -> bytes:
    return (
        f"POST /event HTTP/1.1\r\nHost: localhost\r\nContent-Length: {len(corps_)}\r\n{entetes}\r\n"
    ).encode() + corps_


@pytest.fixture
async def serveur_test(dossier_court, tmp_path):
    recus = []
    socket_ = dossier_court / "etat" / "daemon.sock"
    serveur = await demarrer_serveur(socket_, recus.append, lambda: {"sessions": 2}, tmp_path)
    yield socket_, recus
    serveur.close()
    await serveur.wait_closed()


async def test_post_event(serveur_test):
    socket_, recus = serveur_test
    reponse = await requete(socket_, post(corps(hook_event_name="Stop", session_id="s1"), "X-Claude-Pid: 77\r\n"))
    assert reponse.startswith(b"HTTP/1.1 204")
    assert [(e.nom, e.pid) for e in recus] == [("Stop", 77)]


async def test_expect_100_continue(serveur_test):
    socket_, recus = serveur_test
    reponse = await requete(socket_, post(corps(hook_event_name="Stop", session_id="s1"), "Expect: 100-continue\r\n"))
    assert reponse.startswith(b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 204")
    assert len(recus) == 1


async def test_corps_trop_gros_refuse(serveur_test):
    socket_, recus = serveur_test
    brute = b"POST /event HTTP/1.1\r\nContent-Length: 17000000\r\n\r\n"
    assert (await requete(socket_, brute)).startswith(b"HTTP/1.1 413")
    assert recus == []


async def test_sans_content_length(serveur_test):
    socket_, _ = serveur_test
    assert (await requete(socket_, b"POST /event HTTP/1.1\r\n\r\n")).startswith(b"HTTP/1.1 411")


async def test_route_inconnue(serveur_test):
    socket_, _ = serveur_test
    assert (await requete(socket_, b"GET / HTTP/1.1\r\n\r\n")).startswith(b"HTTP/1.1 404")


async def test_etat(serveur_test):
    socket_, _ = serveur_test
    assert await asyncio.to_thread(interroger_etat, socket_) == {"sessions": 2}


def test_etat_sans_daemon(dossier_court):
    assert interroger_etat(dossier_court / "absent.sock") is None


async def test_permissions(serveur_test):
    socket_, _ = serveur_test
    assert stat.S_IMODE(socket_.stat().st_mode) == 0o600
    assert stat.S_IMODE(socket_.parent.stat().st_mode) == 0o700


async def test_deuxieme_instance_refusee(serveur_test, tmp_path):
    socket_, _ = serveur_test
    with pytest.raises(DejaLance):
        await demarrer_serveur(socket_, lambda e: None, dict, tmp_path)


async def test_socket_orphelin_remplace(dossier_court, tmp_path):
    socket_ = dossier_court / "orphelin.sock"
    socket_.touch()
    serveur = await demarrer_serveur(socket_, lambda e: None, dict, tmp_path)
    serveur.close()
    await serveur.wait_closed()


async def test_erreur_du_gestionnaire_n_arrete_pas_le_serveur(dossier_court, tmp_path):
    socket_ = dossier_court / "d.sock"
    appels = []

    def gestionnaire(ev):
        appels.append(ev)
        if len(appels) == 1:
            raise RuntimeError("boum")

    serveur = await demarrer_serveur(socket_, gestionnaire, dict, tmp_path)
    brute = post(corps(hook_event_name="Stop", session_id="s1"))
    assert (await requete(socket_, brute)).startswith(b"HTTP/1.1 500")
    assert (await requete(socket_, brute)).startswith(b"HTTP/1.1 204")
    serveur.close()
    await serveur.wait_closed()

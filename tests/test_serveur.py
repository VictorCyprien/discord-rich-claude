import asyncio
import json
import logging
import os
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
        b"[" * 100_000,  # F3 : profondeur de récursion, jamais laissée s'échapper comme du JSON valide
        corps(hook_event_name="Stop", session_id="s1\n"),  # F4 : `$` accepte un `\n` final, `fullmatch` non
    ],
)
def test_filtrer_refuse(brut, tmp_path):
    assert filtrer(brut, None, 0.0, tmp_path) is None


@pytest.mark.parametrize(
    "pid_brut,attendu",
    [
        ("123", 123), (None, None), ("abc", None), ("1", None), ("0", None), ("１２", None),
        ("99999999999", None),  # F2 : hors bornes, sans quoi sessions.pid_vivant lève OverflowError
        ("9" * 5000, None),  # F2 : un en-tête déraisonnable ne doit pas même atteindre int()
    ],
)
def test_filtrer_pid(pid_brut, attendu, tmp_path):
    assert filtrer(corps(hook_event_name="Stop", session_id="s1"), pid_brut, 0.0, tmp_path).pid == attendu


@pytest.mark.parametrize(
    "cwd_brut,attendu",
    [
        ("/a\x00b", None),  # F2 : un NUL fait planter carte.nom_projet plus tard
        ("relatif/x", None),  # F2 : un chemin non absolu n'a pas de sens ici
        ("/Users/x/p", "/Users/x/p"),
    ],
)
def test_filtrer_cwd(cwd_brut, attendu, tmp_path):
    ev = filtrer(corps(hook_event_name="Stop", session_id="s1", cwd=cwd_brut), None, 0.0, tmp_path)
    assert ev.cwd == attendu


@pytest.mark.parametrize(
    "agent_id_brut,attendu",
    [
        ("", None),  # F4 : sessions.appliquer traiterait "" comme un vrai sous-agent
        ("a19de871c0abcdef1", "a19de871c0abcdef1"),
        ("../etc", None),
    ],
)
def test_filtrer_agent_id(agent_id_brut, attendu, tmp_path):
    ev = filtrer(
        corps(hook_event_name="Stop", session_id="s1", agent_id=agent_id_brut), None, 0.0, tmp_path
    )
    assert ev.agent_id == attendu


def test_transcript_autorise(tmp_path):
    racine = tmp_path / "projects"
    (racine / "p").mkdir(parents=True)
    assert transcript_autorise(str(racine / "p" / "s1.jsonl"), racine)
    assert not transcript_autorise(str(racine / "p" / "s1.txt"), racine)
    assert not transcript_autorise(str(tmp_path / "ailleurs.jsonl"), racine)
    assert not transcript_autorise(str(racine / ".." / "ailleurs.jsonl"), racine)
    assert not transcript_autorise("relatif/s1.jsonl", racine)


def test_transcript_autorise_faux_sur_nul(tmp_path):
    # F3 : un NUL fait lever ValueError dans resolve(), jamais laissé s'échapper.
    racine = tmp_path / "projects"
    racine.mkdir()
    assert not transcript_autorise("/a\x00b.jsonl", racine)


def test_transcript_autorise_faux_sur_boucle_symlink(tmp_path):
    # F3 : une boucle de liens fait lever OSError/RuntimeError dans resolve().
    racine = tmp_path / "projects"
    (racine / "p").mkdir(parents=True)
    boucle = racine / "p" / "boucle.jsonl"
    boucle.symlink_to(boucle)
    assert not transcript_autorise(str(boucle), racine)


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


# F1 : un lien symbolique au chemin du socket ou du dossier d'état ne doit
# jamais faire créer ou modifier un fichier hors de l'emplacement prévu.


async def test_symlink_au_chemin_socket_est_remplace(dossier_court, tmp_path):
    cible = dossier_court / "ailleurs"  # jamais créée : le lien est pendant
    socket_ = dossier_court / "etat" / "daemon.sock"
    socket_.parent.mkdir(parents=True)
    socket_.symlink_to(cible)
    serveur = await demarrer_serveur(socket_, lambda e: None, dict, tmp_path)
    try:
        info = os.lstat(socket_)
        assert stat.S_ISSOCK(info.st_mode)
        assert not cible.exists()
        assert not cible.is_symlink()
    finally:
        serveur.close()
        await serveur.wait_closed()


async def test_dossier_etat_symlink_refuse(dossier_court, tmp_path):
    ailleurs = dossier_court / "ailleurs"
    ailleurs.mkdir()
    lien = dossier_court / "etat"
    lien.symlink_to(ailleurs)
    socket_ = lien / "daemon.sock"
    with pytest.raises(PermissionError):
        await demarrer_serveur(socket_, lambda e: None, dict, tmp_path)
    assert not (ailleurs / "daemon.sock").exists()


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


async def test_erreur_inattendue_repond_500(dossier_court, tmp_path):
    # F3 : une exception hors du tuple attendu (ex. etat() qui plante) ne doit
    # ni faire planter le serveur, ni laisser la connexion sans réponse.
    socket_ = dossier_court / "d.sock"

    def etat_qui_explose():
        raise TypeError("boum")

    serveur = await demarrer_serveur(socket_, lambda e: None, etat_qui_explose, tmp_path)
    try:
        reponse = await requete(socket_, b"GET /etat HTTP/1.1\r\nHost: localhost\r\n\r\n")
        assert reponse.startswith(b"HTTP/1.1 500")
        # le serveur répond toujours après coup
        brute = post(corps(hook_event_name="Stop", session_id="s1"))
        assert (await requete(socket_, brute)).startswith(b"HTTP/1.1 204")
    finally:
        serveur.close()
        await serveur.wait_closed()


async def test_gestionnaire_erreur_ne_fuite_pas_dans_les_logs(dossier_court, tmp_path, caplog):
    # F3 : ni le message de l'exception ni un chemin ne doivent apparaître dans le log.
    socket_ = dossier_court / "d.sock"
    marqueur = "SECRET-MARQUEUR"
    chemin_secret = "/Users/victor/secret/fichier.py"

    def gestionnaire(ev):
        raise RuntimeError(f"{marqueur} {chemin_secret}")

    serveur = await demarrer_serveur(socket_, gestionnaire, dict, tmp_path)
    try:
        with caplog.at_level(logging.DEBUG):
            brute = post(corps(hook_event_name="Stop", session_id="s1"))
            assert (await requete(socket_, brute)).startswith(b"HTTP/1.1 500")
            assert (await requete(socket_, brute)).startswith(b"HTTP/1.1 500")
        for enregistrement in caplog.records:
            assert marqueur not in enregistrement.getMessage()
            assert chemin_secret not in enregistrement.getMessage()
            assert marqueur not in (enregistrement.exc_text or "")
            assert chemin_secret not in (enregistrement.exc_text or "")
    finally:
        serveur.close()
        await serveur.wait_closed()

import asyncio
import json
import logging
import os
import shutil
import struct
import tempfile
import time
from contextlib import suppress

import pytest

import discord_rich_claude.presence as presence_module
from discord_rich_claude.carte import Carte
from discord_rich_claude.presence import ClientPypresence, Presence

A = Carte(details="Écrit du code · Projet privé", state="Opus 5 · 1 k tokens", debut=0)
B = Carte(details="Lit le code · Projet privé", state="Opus 5 · 1 k tokens", debut=0)
C = Carte(details="Attend ta réponse · Projet privé", state="Opus 5 · 1 k tokens", debut=0)


class Horloge:
    def __init__(self):
        self.t = 0.0

    def __call__(self):
        return self.t


class FauxClient:
    def __init__(self, journal, client_id, echec_connexion=False, echec_publication=False):
        self.journal, self.client_id = journal, client_id
        self.echec_connexion, self.echec_publication = echec_connexion, echec_publication

    async def connecter(self):
        self.journal.append(("connecter", self.client_id))
        if self.echec_connexion:
            raise ConnectionRefusedError("Discord fermé")

    async def publier(self, carte):
        if self.echec_publication:
            self.journal.append("publier-echec")
            raise BrokenPipeError("pipe fermé")
        self.journal.append(("publier", carte.details))

    async def effacer(self):
        self.journal.append("effacer")

    async def fermer(self):
        self.journal.append("fermer")


def presence(journal, horloge, **echecs):
    return Presence(
        lambda cid: FauxClient(journal, cid, **echecs),
        intervalle_min_s=15,
        reessai_s=15,
        horloge=horloge,
    )


async def test_premiere_carte_connecte_et_publie():
    journal, h = [], Horloge()
    p = presence(journal, h)
    await p.soumettre(A, "42")
    assert journal == [("connecter", "42"), ("publier", A.details)]
    assert p.connecte


async def test_meme_carte_pas_republiee():
    journal, h = [], Horloge()
    p = presence(journal, h)
    await p.soumettre(A, "42")
    h.t = 100
    await p.soumettre(A, "42")
    assert journal.count(("publier", A.details)) == 1


async def test_rafale_regroupee_la_derniere_gagne():
    journal, h = [], Horloge()
    p = presence(journal, h)
    await p.soumettre(A, "42")
    h.t = 5
    await p.soumettre(B, "42")
    h.t = 10
    await p.soumettre(C, "42")
    assert ("publier", B.details) not in journal
    h.t = 15
    await p.soumettre(C, "42")
    assert journal[-1] == ("publier", C.details)
    assert ("publier", B.details) not in journal


async def test_plus_de_carte_efface_et_ferme_puis_reconnecte():
    journal, h = [], Horloge()
    p = presence(journal, h)
    await p.soumettre(A, "42")
    await p.soumettre(None, "42")
    assert journal[-2:] == ["effacer", "fermer"]
    assert not p.connecte
    await p.soumettre(None, "42")
    assert journal.count("fermer") == 1
    h.t = 20
    await p.soumettre(A, "42")
    assert journal[-2:] == [("connecter", "42"), ("publier", A.details)]


async def test_sans_client_id_rien_ne_se_connecte():
    journal, h = [], Horloge()
    p = presence(journal, h)
    await p.soumettre(A, None)
    assert journal == []


async def test_connexion_refusee_reessai_apres_15_s():
    journal, h = [], Horloge()
    p = presence(journal, h, echec_connexion=True)
    await p.soumettre(A, "42")
    h.t = 10
    await p.soumettre(A, "42")
    assert journal == [("connecter", "42")]
    h.t = 15
    await p.soumettre(A, "42")
    assert journal == [("connecter", "42"), ("connecter", "42")]


async def test_publication_refusee_abandonne_puis_reessaie():
    journal, h = [], Horloge()
    p = presence(journal, h, echec_publication=True)
    await p.soumettre(A, "42")
    assert journal[-2:] == ["publier-echec", "fermer"]
    assert not p.connecte
    h.t = 10
    await p.soumettre(A, "42")
    assert journal.count(("connecter", "42")) == 1
    h.t = 15
    await p.soumettre(A, "42")
    assert journal.count(("connecter", "42")) == 2


async def test_changement_de_client_id_reconnecte():
    journal, h = [], Horloge()
    p = presence(journal, h)
    await p.soumettre(A, "42")
    h.t = 20
    await p.soumettre(A, "43")
    assert journal[-4:] == ["effacer", "fermer", ("connecter", "43"), ("publier", A.details)]


async def test_echecs_de_connexion_repetes_un_seul_warning_puis_debug(caplog):
    """F3 : premier échec d'un type d'exception donné → WARNING (le type seul,
    jamais str(e)) ; les échecs suivants du même type → DEBUG ; un changement de
    type d'exception (toujours sans succès entre les deux) → un nouveau WARNING ;
    la reconnexion réussie déclenche l'INFO déjà en place (« connecté à Discord »)."""

    class ClientEchecPuisSucces:
        def __init__(self, journal, client_id, exceptions):
            self.journal, self.client_id, self._exceptions = journal, client_id, exceptions

        async def connecter(self):
            self.journal.append(("connecter", self.client_id))
            if self._exceptions:
                raise self._exceptions.pop(0)

        async def publier(self, carte):
            self.journal.append(("publier", carte.details))

        async def effacer(self):
            self.journal.append("effacer")

        async def fermer(self):
            self.journal.append("fermer")

    exceptions = [ConnectionRefusedError("Discord fermé")] * 5 + [TimeoutError("trop long")]
    journal, h = [], Horloge()
    p = Presence(
        lambda cid: ClientEchecPuisSucces(journal, cid, exceptions),
        intervalle_min_s=15,
        reessai_s=0,
        horloge=h,
    )

    with caplog.at_level(logging.DEBUG, logger="discord_rich_claude.presence"):
        for i in range(7):  # 5 x ConnectionRefusedError, 1 x TimeoutError, puis succès
            h.t = i
            await p.soumettre(A, "42")

    avertissements = [r for r in caplog.records if r.levelno == logging.WARNING]
    debogages = [r for r in caplog.records if r.levelno == logging.DEBUG]
    infos = [r for r in caplog.records if r.levelno == logging.INFO]

    assert len(avertissements) == 2
    assert "ConnectionRefusedError" in avertissements[0].getMessage()
    assert "TimeoutError" in avertissements[1].getMessage()
    assert len(debogages) == 4  # les 4 répétitions de ConnectionRefusedError

    for record in avertissements + debogages:
        assert "Discord fermé" not in record.getMessage()
        assert "trop long" not in record.getMessage()

    assert any("connecté à Discord" in r.getMessage() for r in infos)
    assert p.connecte


@pytest.mark.filterwarnings("ignore::pytest.PytestUnraisableExceptionWarning")
async def test_client_pypresence_ferme_sans_fermer_la_boucle(monkeypatch):
    """ClientPypresence.fermer() contre un faux serveur IPC Discord (aucun réseau,
    aucune vraie app Discord). Vérifie que fermer() envoie la trame de fermeture
    (op 2), ferme le socket écrivain de son côté (le serveur voit un EOF), et ne
    touche jamais à la boucle asyncio en cours.

    Le filtre ignore un avertissement propre à pypresence 4.6.2 : `get_ipc_path()`
    (pypresence/utils.py) fait `return` depuis l'intérieur d'un `for entry in
    os.scandir(...)`, sans jamais fermer l'itérateur ; sous `-W error`, le
    `ResourceWarning: unclosed scandir iterator` levé à sa finalisation devient une
    erreur. Défaut de la librairie tierce, sans rapport avec `fermer()` ; hors
    périmètre de ce correctif."""
    dossier = tempfile.mkdtemp(dir="/tmp")
    monkeypatch.setenv("XDG_RUNTIME_DIR", dossier)
    chemin_socket = os.path.join(dossier, "discord-ipc-0")
    recu = []
    fermeture = asyncio.Event()

    async def gerer_connexion(reader, writer):
        try:
            while True:
                try:
                    entete = await reader.readexactly(8)
                except asyncio.IncompleteReadError:
                    return
                op, longueur = struct.unpack("<II", entete)
                corps = await reader.readexactly(longueur) if longueur else b""
                charge = json.loads(corps) if corps else {}
                recu.append((op, charge))
                if op == 0:
                    reponse = json.dumps({"cmd": "DISPATCH", "evt": "READY", "data": {}}).encode()
                    writer.write(struct.pack("<II", 1, len(reponse)) + reponse)
                    await writer.drain()
                elif op == 1:
                    reponse = json.dumps({"cmd": "SET_ACTIVITY", "evt": None, "data": {}}).encode()
                    writer.write(struct.pack("<II", 1, len(reponse)) + reponse)
                    await writer.drain()
                elif op == 2:
                    fin = await asyncio.wait_for(reader.read(1), 1)
                    recu.append(("eof", fin))
                    fermeture.set()
                    return
        finally:
            writer.close()

    serveur = await asyncio.start_unix_server(gerer_connexion, path=chemin_socket)
    try:
        carte = Carte(details="Écrit du code · Projet privé", state="Opus 5 · 1 k tokens", debut=1234)
        client = ClientPypresence("42")
        await client.connecter()
        await client.publier(carte)
        await client.effacer()
        await client.fermer()
        await asyncio.wait_for(fermeture.wait(), timeout=2)
    finally:
        serveur.close()
        await serveur.wait_closed()
        shutil.rmtree(dossier, ignore_errors=True)

    ops = [op for op, _ in recu if isinstance(op, int)]
    assert ops == [0, 1, 1, 2]

    _, poignee_de_main = recu[0]
    assert poignee_de_main["client_id"] == "42"

    _, publication = recu[1]
    activite = publication["args"]["activity"]
    assert activite["details"] == carte.details
    assert activite["state"] == carte.state
    assert activite["timestamps"]["start"] == carte.debut
    assert activite["assets"]["large_image"] == carte.large_image
    assert activite["assets"]["large_text"] == carte.large_text

    assert recu[-1] == ("eof", b"")
    assert not asyncio.get_running_loop().is_closed()


async def test_fermeture_echouee_loggee_en_warning(caplog):
    """Une étape de fermeture qui échoue (ici l'envoi de la trame de fermeture,
    op 2) est loggée en WARNING, jamais silencieuse, et fermer() ne relève rien."""
    client = ClientPypresence("42")

    def echoue(*args, **kwargs):
        raise BrokenPipeError("pipe fermé")

    client._rpc.send_data = echoue

    with caplog.at_level(logging.WARNING, logger="discord_rich_claude.presence"):
        await client.fermer()

    avertissements = [r for r in caplog.records if r.levelno == logging.WARNING]
    assert len(avertissements) == 1
    assert "BrokenPipeError" in avertissements[0].getMessage()
    assert "pipe fermé" not in avertissements[0].getMessage()


@pytest.mark.filterwarnings("ignore::pytest.PytestUnraisableExceptionWarning")
async def test_connecter_sans_reponse_expire_et_ferme_le_socket(monkeypatch):
    """G4 : pypresence 4.6.2 borne l'ouverture du socket mais pas la lecture de la
    réponse à la poignée de main (BaseClient.handshake). Un pair qui accepte la
    connexion sans jamais répondre ne doit pas bloquer connecter() indéfiniment, et le
    socket doit être refermé de notre côté (le serveur voit un EOF).

    Le filtre ignore le même avertissement propre à pypresence 4.6.2 que
    `test_client_pypresence_ferme_sans_fermer_la_boucle` (get_ipc_path() sonde la
    connectivité du socket trouvé par scandir avant handshake() ; sous -W error,
    l'itérateur scandir non fermé de la librairie tierce lève une erreur à sa
    finalisation). Sans rapport avec ce correctif."""
    monkeypatch.setattr(presence_module, "DELAI_POIGNEE_MAIN_S", 0.2)
    dossier = tempfile.mkdtemp(dir="/tmp")
    monkeypatch.setenv("XDG_RUNTIME_DIR", dossier)
    chemin_socket = os.path.join(dossier, "discord-ipc-0")
    fermeture = asyncio.Event()
    recu = []

    async def gerer_connexion(reader, writer):
        try:
            try:
                entete = await reader.readexactly(8)
            except asyncio.IncompleteReadError:
                return  # sonde de connectivité de get_ipc_path, pas la poignée de main
            _, longueur = struct.unpack("<II", entete)
            await reader.readexactly(longueur)
            try:
                fin = await reader.read(1)  # ne répond jamais : attend que le client ferme
            except (asyncio.IncompleteReadError, ConnectionError):
                fin = b""
            recu.append(fin)
            fermeture.set()
        finally:
            writer.close()

    serveur = await asyncio.start_unix_server(gerer_connexion, path=chemin_socket)
    try:
        client = ClientPypresence("42")
        debut = time.monotonic()
        with pytest.raises(Exception):
            await client.connecter()
        duree = time.monotonic() - debut
        await asyncio.wait_for(fermeture.wait(), timeout=2)
    finally:
        serveur.close()
        # Si le socket n'a pas été refermé côté client, la connexion acceptée reste
        # ouverte et wait_closed() ne revient jamais (Python 3.12) : on la borne pour
        # que l'absence du correctif se traduise par un échec net, pas un blocage.
        with suppress(TimeoutError):
            await asyncio.wait_for(serveur.wait_closed(), timeout=2)
        shutil.rmtree(dossier, ignore_errors=True)

    assert duree < 2  # borné par DELAI_POIGNEE_MAIN_S (0,2 s ici), pas connection_timeout=5
    assert recu == [b""]


@pytest.mark.filterwarnings("ignore::pytest.PytestUnraisableExceptionWarning")
async def test_connecter_id_invalide_leve_et_ferme_le_socket(monkeypatch):
    """G4 : un pair qui répond par une trame d'erreur (ID de client invalide) puis
    garde le tuyau ouvert ne doit pas fuir le descripteur : connecter() referme le
    socket de son côté avant de relever, le serveur voit un EOF.

    Même filtre que ci-dessus, pour le même avertissement propre à pypresence 4.6.2,
    sans rapport avec ce correctif."""
    dossier = tempfile.mkdtemp(dir="/tmp")
    monkeypatch.setenv("XDG_RUNTIME_DIR", dossier)
    chemin_socket = os.path.join(dossier, "discord-ipc-0")
    fermeture = asyncio.Event()
    recu = []

    async def gerer_connexion(reader, writer):
        try:
            try:
                entete = await reader.readexactly(8)
            except asyncio.IncompleteReadError:
                return  # sonde de connectivité de get_ipc_path, pas la poignée de main
            _, longueur = struct.unpack("<II", entete)
            await reader.readexactly(longueur)
            erreur = json.dumps({"code": 4000, "message": "Invalid Client ID"}).encode()
            writer.write(struct.pack("<II", 1, len(erreur)) + erreur)
            await writer.drain()
            try:
                fin = await reader.read(1)
            except (asyncio.IncompleteReadError, ConnectionError):
                fin = b""
            recu.append(fin)
            fermeture.set()
        finally:
            writer.close()

    serveur = await asyncio.start_unix_server(gerer_connexion, path=chemin_socket)
    try:
        client = ClientPypresence("42")
        with pytest.raises(Exception):
            await client.connecter()
        await asyncio.wait_for(fermeture.wait(), timeout=2)
    finally:
        serveur.close()
        with suppress(TimeoutError):
            await asyncio.wait_for(serveur.wait_closed(), timeout=2)
        shutil.rmtree(dossier, ignore_errors=True)

    assert recu == [b""]

import asyncio
import json
import os
import shutil
import struct
import tempfile

import pytest

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

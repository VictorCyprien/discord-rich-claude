from discord_rich_claude.carte import Carte
from discord_rich_claude.presence import Presence

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

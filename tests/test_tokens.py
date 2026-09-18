import json
import os
import time
from datetime import datetime, timedelta, timezone

import pytest

from discord_rich_claude.tokens import SuiviTokens, fichiers_de_session, minuit_local

MAINTENANT = time.time()
MINUIT = minuit_local(MAINTENANT)


def iso(t):
    return datetime.fromtimestamp(t).astimezone().isoformat()


def ligne(msg_id, req="r1", inp=0, out=0, cw=0, cr=0, t=None, model="claude-opus-5", typ="assistant"):
    return json.dumps(
        {
            "type": typ,
            "requestId": req,
            "timestamp": iso(MAINTENANT if t is None else t),
            "message": {
                "id": msg_id,
                "model": model,
                "usage": {
                    "input_tokens": inp,
                    "output_tokens": out,
                    "cache_creation_input_tokens": cw,
                    "cache_read_input_tokens": cr,
                },
            },
        }
    ) + "\n"


def session(racine, sid="s1", lignes=(), sous_agents=None):
    t = racine / "projet" / f"{sid}.jsonl"
    t.parent.mkdir(parents=True, exist_ok=True)
    t.write_text("".join(lignes), encoding="utf-8")
    for nom, contenu in (sous_agents or {}).items():
        f = racine / "projet" / sid / "subagents" / f"{nom}.jsonl"
        f.parent.mkdir(parents=True, exist_ok=True)
        f.write_text("".join(contenu), encoding="utf-8")
    return t


def test_exclut_les_lectures_du_cache(tmp_path):
    t = session(tmp_path, lignes=[ligne("m1", inp=10, out=5, cw=100, cr=10_000)])
    suivi = SuiviTokens(tmp_path)
    suivi.rafraichir([t], MINUIT)
    assert suivi.tokens_session(t) == 115


def test_doublons_a_valeurs_differentes_gardent_le_maximum(tmp_path):
    t = session(tmp_path, lignes=[ligne("m1", inp=10, out=5), ligne("m1", inp=10, out=50)])
    suivi = SuiviTokens(tmp_path)
    suivi.rafraichir([t], MINUIT)
    assert suivi.tokens_session(t) == 60


def test_sous_agents_inclus_et_dedoublonnes_entre_fichiers(tmp_path):
    t = session(
        tmp_path,
        lignes=[ligne("m1", out=10)],
        sous_agents={"agent-a": [ligne("m2", out=20), ligne("m1", out=10)]},
    )
    suivi = SuiviTokens(tmp_path)
    suivi.rafraichir([t], MINUIT)
    assert suivi.tokens_session(t) == 30


def test_ligne_incomplete_attend_le_tour_suivant(tmp_path):
    t = session(tmp_path, lignes=[ligne("m1", out=10)])
    with t.open("a", encoding="utf-8") as f:
        f.write(ligne("m2", out=7).rstrip("\n"))
    suivi = SuiviTokens(tmp_path)
    suivi.rafraichir([t], MINUIT)
    assert suivi.tokens_session(t) == 10
    with t.open("a", encoding="utf-8") as f:
        f.write("\n")
    suivi.rafraichir([t], MINUIT)
    assert suivi.tokens_session(t) == 17


def test_ligne_illisible_ignoree(tmp_path):
    t = session(tmp_path, lignes=["{pas du json\n", ligne("m1", out=3)])
    suivi = SuiviTokens(tmp_path)
    suivi.rafraichir([t], MINUIT)
    assert suivi.tokens_session(t) == 3


def test_fichier_reecrit_plus_court_relu_depuis_le_debut(tmp_path):
    t = session(tmp_path, lignes=[ligne("m1", out=10), ligne("m2", out=10)])
    suivi = SuiviTokens(tmp_path)
    suivi.rafraichir([t], MINUIT)
    t.write_text(ligne("m3", out=4), encoding="utf-8")
    suivi.rafraichir([t], MINUIT)
    assert suivi.tokens_session(t) == 4


def test_cumul_du_jour_commence_a_minuit(tmp_path):
    hier = MINUIT - 3600
    t = session(tmp_path, lignes=[ligne("m1", out=100, t=hier), ligne("m2", out=5)])
    autre = session(tmp_path, sid="s2", lignes=[ligne("m3", out=7)])
    suivi = SuiviTokens(tmp_path)
    suivi.rafraichir([t], MINUIT)
    assert suivi.tokens_session(t) == 105
    assert suivi.tokens_jour(MINUIT) == 12  # m2 + m3, s2 trouvé par sa date de modification
    assert autre in suivi.fichiers_suivis


def test_fichier_ancien_hors_session_oublie(tmp_path):
    t = session(tmp_path, lignes=[ligne("m1", out=1, t=MINUIT - 86_400)])
    vieux = MINUIT - 86_400
    os.utime(t, (vieux, vieux))
    suivi = SuiviTokens(tmp_path)
    suivi.rafraichir([t], MINUIT)
    assert t in suivi.fichiers_suivis
    suivi.rafraichir([], MINUIT)
    assert t not in suivi.fichiers_suivis


def test_modele_du_dernier_message_assistant(tmp_path):
    t = session(
        tmp_path,
        lignes=[
            ligne("m1", model="claude-sonnet-5"),
            ligne("m2", model="claude-opus-5"),
            ligne("m3", model="<synthetic>"),
            ligne("m4", model="claude-haiku-4-5", typ="user"),
        ],
    )
    suivi = SuiviTokens(tmp_path)
    suivi.rafraichir([t], MINUIT)
    assert suivi.modele(t) == "claude-opus-5"


def test_session_inconnue_vaut_zero(tmp_path):
    suivi = SuiviTokens(tmp_path)
    assert suivi.tokens_session(tmp_path / "absent.jsonl") == 0
    assert suivi.modele(tmp_path / "absent.jsonl") is None


def test_fichiers_de_session(tmp_path):
    t = session(tmp_path, sous_agents={"agent-b": [], "agent-a": []})
    dossier = tmp_path / "projet" / "s1" / "subagents"
    assert fichiers_de_session(t) == [t, dossier / "agent-a.jsonl", dossier / "agent-b.jsonl"]


def test_minuit_local():
    m = minuit_local(MAINTENANT)
    d = datetime.fromtimestamp(m)
    assert (d.hour, d.minute, d.second) == (0, 0, 0)
    assert timedelta(0) <= datetime.fromtimestamp(MAINTENANT) - d < timedelta(hours=25)


@pytest.fixture
def tz_paris():
    """Force le fuseau Europe/Paris pour la durée du test, restauré ensuite.

    Restauration manuelle (plutôt que monkeypatch.setenv) : monkeypatch ne
    rappelle jamais time.tzset() après avoir annulé la variable TZ, ce qui
    laisserait le fuseau du process bloqué sur Europe/Paris pour les tests
    suivants. Ici la restauration de TZ et le second tzset() sont dans le
    même bloc, sans dépendre de l'ordre de démontage d'une autre fixture.
    """
    original = os.environ.get("TZ")
    os.environ["TZ"] = "Europe/Paris"
    time.tzset()
    try:
        yield
    finally:
        if original is None:
            os.environ.pop("TZ", None)
        else:
            os.environ["TZ"] = original
        time.tzset()


def test_minuit_local_passage_heure_ete(tz_paris):
    # 2026-03-29 : passage à l'heure d'été à 02h00 -> 03h00 local. Minuit est
    # encore à l'ancien décalage (CET, UTC+1), pas celui de "maintenant" (CEST, UTC+2).
    maintenant = datetime(2026, 3, 29, 14, 0, 0).timestamp()
    attendu = datetime(2026, 3, 28, 23, 0, 0, tzinfo=timezone.utc).timestamp()
    assert minuit_local(maintenant) == attendu


def test_minuit_local_passage_heure_hiver(tz_paris):
    # 2026-10-25 : passage à l'heure d'hiver à 03h00 -> 02h00 local. Minuit est
    # encore à l'ancien décalage (CEST, UTC+2), pas celui de "maintenant" (CET, UTC+1).
    maintenant = datetime(2026, 10, 25, 14, 0, 0).timestamp()
    attendu = datetime(2026, 10, 24, 22, 0, 0, tzinfo=timezone.utc).timestamp()
    assert minuit_local(maintenant) == attendu


def test_minuit_local_jour_ordinaire_tz_paris(tz_paris):
    maintenant = datetime(2026, 1, 15, 10, 0, 0).timestamp()
    attendu = datetime(2026, 1, 14, 23, 0, 0, tzinfo=timezone.utc).timestamp()
    assert minuit_local(maintenant) == attendu


def test_fichier_illisible_ignore_les_autres_restent_comptes(tmp_path):
    if os.geteuid() == 0:
        pytest.skip("chmod sans effet pour root")
    lisible = session(tmp_path, sid="s1", lignes=[ligne("m1", out=5)])
    illisible = session(tmp_path, sid="s2", lignes=[ligne("m2", out=9)])
    illisible.chmod(0o000)
    try:
        suivi = SuiviTokens(tmp_path)
        suivi.rafraichir([lisible, illisible], MINUIT)
        assert suivi.tokens_session(lisible) == 5
    finally:
        illisible.chmod(0o644)

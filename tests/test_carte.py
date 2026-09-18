from pathlib import Path

import pytest

import discord_rich_claude.carte as carte_module
from discord_rich_claude.carte import (
    Carte,
    Stats,
    construire_carte,
    est_vue_b,
    formater_nombre,
    nom_modele,
    nom_projet,
)
from discord_rich_claude.config import Config
from discord_rich_claude.sessions import Evenement, Registre


@pytest.mark.parametrize(
    "n,attendu",
    [
        (0, "0"),
        (850, "850"),
        (999, "999"),
        (1_000, "1 k"),
        (445_439, "445 k"),
        (999_999, "999 k"),
        (1_000_000, "1,0 M"),
        (7_721_871, "7,7 M"),
    ],
)
def test_formater_nombre(n, attendu):
    assert formater_nombre(n) == attendu


@pytest.mark.parametrize(
    "model_id,attendu",
    [
        ("claude-opus-5", "Opus 5"),
        ("claude-sonnet-5", "Sonnet 5"),
        ("claude-sonnet-4-6", "Sonnet 4.6"),
        ("claude-haiku-4-5-20251001", "Haiku 4.5"),
        ("claude-fable-5-1", "Fable 5.1"),
        ("claude-mystere-9", "mystere-9"),
        ("gpt-autre", "gpt-autre"),
        (None, None),
        ("", None),
    ],
)
def test_nom_modele(model_id, attendu):
    assert nom_modele(model_id) == attendu


def test_nom_projet_autorise_exact(tmp_path):
    p = tmp_path / "MonProjet"
    p.mkdir()
    assert nom_projet(str(p), (p.resolve(),)) == "MonProjet"


def test_nom_projet_sous_dossier_et_worktree(tmp_path):
    p = tmp_path / "MonProjet"
    wt = p / ".claude" / "worktrees" / "feature-x"
    wt.mkdir(parents=True)
    assert nom_projet(str(wt), (p.resolve(),)) == "MonProjet"


def test_nom_projet_homonyme_non_autorise(tmp_path):
    autorise = tmp_path / "perso" / "Projet"
    client = tmp_path / "client" / "Projet"
    autorise.mkdir(parents=True)
    client.mkdir(parents=True)
    assert nom_projet(str(client), (autorise.resolve(),)) == "Projet privé"


def test_nom_projet_prefixe_de_nom_non_autorise(tmp_path):
    autorise = tmp_path / "app"
    voisin = tmp_path / "app-client"
    autorise.mkdir()
    voisin.mkdir()
    assert nom_projet(str(voisin), (autorise.resolve(),)) == "Projet privé"


def test_nom_projet_liste_vide_ou_cwd_absent(tmp_path):
    assert nom_projet(str(tmp_path), ()) == "Projet privé"
    assert nom_projet(None, (tmp_path.resolve(),)) == "Projet privé"


def test_est_vue_b_alterne():
    assert [est_vue_b(t, 20) for t in (0, 19.9, 20, 39.9, 40)] == [False, False, True, True, False]


def registre(*sessions):
    r = Registre()
    for sid, debut, cwd in sessions:
        r.appliquer(Evenement("SessionStart", sid, debut, cwd=cwd))
    return r


def test_carte_vue_a(tmp_path):
    p = tmp_path / "MonProjet"
    p.mkdir()
    r = registre(("s1", 1000, str(p)))
    r.appliquer(Evenement("PreToolUse", "s1", 1010, tool_name="Edit"))
    cfg = Config(client_id="1", projets_autorises=(p.resolve(),))
    carte = construire_carte(r, cfg, Stats("claude-opus-5", 445_439, 8_400_000), 1011, vue_b=False)
    assert carte == Carte(details="Écrit du code · MonProjet", state="Opus 5 · 445 k tokens", debut=1000)


def test_carte_vue_b_pluriel(tmp_path):
    r = registre(("s1", 1000, None), ("s2", 900, None), ("s3", 950, None))
    carte = construire_carte(r, Config(), Stats(None, 0, 8_400_000), 1001, vue_b=True)
    assert carte.state == "3 sessions · 8,4 M aujourd'hui"
    assert carte.debut == 900
    assert carte.details.endswith(" · Projet privé")


def test_carte_vue_b_singulier():
    r = registre(("s1", 1000, None))
    carte = construire_carte(r, Config(), Stats(None, 0, 120), 1001, vue_b=True)
    assert carte.state == "1 session · 120 aujourd'hui"


def test_carte_sans_modele():
    r = registre(("s1", 1000, None))
    carte = construire_carte(r, Config(), Stats(None, 445_439, 0), 1001, vue_b=False)
    assert carte.state == "445 k tokens"


def test_carte_inactive():
    r = registre(("s1", 1000, None))
    carte = construire_carte(r, Config(inactif_apres_min=10), Stats(), 1000 + 600, vue_b=False)
    assert carte.details == "Inactif · Projet privé"


def test_pas_de_carte_sans_session():
    assert construire_carte(Registre(), Config(), Stats(), 0, vue_b=False) is None


# --- Fix round 2 (final-fix wave) : F4 -- résolution de cwd mise en cache ---


def test_nom_projet_resolution_mise_en_cache(tmp_path, monkeypatch):
    p = tmp_path / "MonProjet"
    p.mkdir()
    autorise = p.resolve()
    carte_module._resoudre.cache_clear()
    compteur = {"n": 0}
    resolve_original = Path.resolve

    def resolve_compte(self, *a, **kw):
        compteur["n"] += 1
        return resolve_original(self, *a, **kw)

    monkeypatch.setattr(Path, "resolve", resolve_compte)
    assert nom_projet(str(p), (autorise,)) == "MonProjet"
    assert nom_projet(str(p), (autorise,)) == "MonProjet"
    assert compteur["n"] == 1


# --- Fix round 2 (final-fix wave) : F5 -- limite Discord de 128 caractères ---


def test_details_tronque_a_128_caracteres(tmp_path):
    nom = "x" * 200
    p = tmp_path / nom
    p.mkdir()
    r = registre(("s1", 1000, str(p)))
    carte = construire_carte(r, Config(projets_autorises=(p.resolve(),)), Stats(), 1001, vue_b=False)
    assert len(carte.details) == 128
    assert carte.details.endswith("…")


def test_state_tronque_a_128_caracteres():
    r = registre(("s1", 1000, None))
    modele_long = "x" * 200
    carte = construire_carte(r, Config(), Stats(modele=modele_long), 1001, vue_b=False)
    assert len(carte.state) == 128
    assert carte.state.endswith("…")

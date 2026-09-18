import json
from datetime import datetime

import pytest

from discord_rich_claude.chemins import COMMANDE_HOOK
from discord_rich_claude.installation import (
    EVENEMENTS_HOOKS,
    ajouter_hooks,
    modifier_settings,
    retirer_hooks,
)

TIERS = {"type": "command", "command": "afplay /System/Library/Sounds/Glass.aiff"}


def settings_tiers():
    return {
        "model": "opus",
        "hooks": {
            "Stop": [{"hooks": [dict(TIERS)]}],
            "PostToolUse": [{"matcher": "Edit", "hooks": [dict(TIERS)]}],
        },
    }


def nos_hooks(settings, ev):
    return [
        h
        for g in settings["hooks"].get(ev, [])
        for h in g["hooks"]
        if "discord-rich-claude" in h.get("command", "")
    ]


def test_ajouter_pose_les_8_evenements():
    s = ajouter_hooks({})
    assert set(s["hooks"]) == set(EVENEMENTS_HOOKS)
    assert len(EVENEMENTS_HOOKS) == 8
    for ev in EVENEMENTS_HOOKS:
        assert nos_hooks(s, ev) == [{"type": "command", "command": COMMANDE_HOOK, "timeout": 2}]
    assert s["hooks"]["PreToolUse"][0]["matcher"] == "*"


def test_ajouter_conserve_les_hooks_tiers_et_ne_modifie_pas_l_entree():
    avant = settings_tiers()
    copie = json.loads(json.dumps(avant))
    s = ajouter_hooks(avant)
    assert avant == copie
    assert s["model"] == "opus"
    assert s["hooks"]["Stop"][0] == {"hooks": [TIERS]}
    assert s["hooks"]["PostToolUse"] == [{"matcher": "Edit", "hooks": [TIERS]}]


def test_ajouter_est_idempotent():
    une_fois = ajouter_hooks(settings_tiers())
    assert ajouter_hooks(une_fois) == une_fois


def test_retirer_ne_retire_que_les_notres():
    assert retirer_hooks(ajouter_hooks(settings_tiers())) == settings_tiers()


def test_retirer_d_un_groupe_mixte_garde_le_hook_tiers():
    s = {"hooks": {"Stop": [{"hooks": [dict(TIERS), {"type": "command", "command": COMMANDE_HOOK}]}]}}
    assert retirer_hooks(s) == {"hooks": {"Stop": [{"hooks": [TIERS]}]}}


def test_retirer_sans_hooks_ne_change_rien():
    assert retirer_hooks({"model": "opus"}) == {"model": "opus"}
    assert retirer_hooks({"hooks": {"Stop": []}}) == {"hooks": {"Stop": []}}


def horloge():
    return datetime(2026, 9, 18, 14, 30, 5)


def test_modifier_settings_sauvegarde_puis_ecrit(tmp_path):
    f = tmp_path / "settings.json"
    f.write_text(json.dumps(settings_tiers()), encoding="utf-8")
    diffs = []
    res = modifier_settings(f, ajouter_hooks, lambda d: diffs.append(d) or True, horloge)
    assert res == "modifié"
    assert json.loads(f.read_text()) == ajouter_hooks(settings_tiers())
    sauvegarde = tmp_path / "settings.json.bak-20260918-143005"
    assert json.loads(sauvegarde.read_text()) == settings_tiers()
    assert "discord-rich-claude" in diffs[0]


def test_modifier_settings_refuse_n_ecrit_rien(tmp_path):
    f = tmp_path / "settings.json"
    f.write_text(json.dumps(settings_tiers()), encoding="utf-8")
    assert modifier_settings(f, ajouter_hooks, lambda d: False, horloge) == "refusé"
    assert json.loads(f.read_text()) == settings_tiers()
    assert list(tmp_path.iterdir()) == [f]


def test_modifier_settings_inchange(tmp_path):
    f = tmp_path / "settings.json"
    f.write_text(json.dumps({"model": "opus"}), encoding="utf-8")
    assert modifier_settings(f, retirer_hooks, lambda d: True, horloge) == "inchangé"
    assert list(tmp_path.iterdir()) == [f]


def test_modifier_settings_fichier_absent(tmp_path):
    f = tmp_path / "settings.json"
    assert modifier_settings(f, ajouter_hooks, lambda d: True, horloge) == "modifié"
    assert json.loads(f.read_text()) == ajouter_hooks({})


@pytest.mark.parametrize("contenu", ["{pas du json", "[1, 2]"])
def test_modifier_settings_invalide_leve_sans_ecrire(tmp_path, contenu):
    f = tmp_path / "settings.json"
    f.write_text(contenu, encoding="utf-8")
    with pytest.raises(ValueError):
        modifier_settings(f, ajouter_hooks, lambda d: True, horloge)
    assert f.read_text() == contenu

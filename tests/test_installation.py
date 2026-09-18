import inspect
import json
import os
import plistlib
import stat
import subprocess
from datetime import datetime

import pytest

from discord_rich_claude.chemins import COMMANDE_HOOK, LABEL
from discord_rich_claude.installation import (
    EVENEMENTS_HOOKS,
    ajouter_hooks,
    desinstaller_launch_agent,
    generer_plist,
    installer_launch_agent,
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


# --- Fix round 1 : H1 -- édition concurrente pendant la confirmation ---


def test_modifier_settings_detecte_changement_concurrent(tmp_path):
    f = tmp_path / "settings.json"
    f.write_text(json.dumps(settings_tiers()), encoding="utf-8")

    def confirmer_puis_modifier_ailleurs(diff):
        f.write_text(json.dumps({"model": "sonnet"}), encoding="utf-8")
        return True

    with pytest.raises(ValueError):
        modifier_settings(f, ajouter_hooks, confirmer_puis_modifier_ailleurs, horloge)
    assert json.loads(f.read_text()) == {"model": "sonnet"}
    assert {p.name for p in tmp_path.iterdir()} == {"settings.json"}


def test_modifier_settings_detecte_apparition_du_fichier(tmp_path):
    f = tmp_path / "settings.json"

    def confirmer_puis_creer(diff):
        f.write_text(json.dumps({"model": "sonnet"}), encoding="utf-8")
        return True

    with pytest.raises(ValueError):
        modifier_settings(f, ajouter_hooks, confirmer_puis_creer, horloge)
    assert json.loads(f.read_text()) == {"model": "sonnet"}
    assert {p.name for p in tmp_path.iterdir()} == {"settings.json"}


# --- Fix round 1 : H2 -- droits du fichier, écriture atomique, sauvegarde exclusive ---


def test_modifier_settings_conserve_les_droits(tmp_path):
    f = tmp_path / "settings.json"
    f.write_text(json.dumps(settings_tiers()), encoding="utf-8")
    f.chmod(0o600)
    assert modifier_settings(f, ajouter_hooks, lambda d: True, horloge) == "modifié"
    assert stat.S_IMODE(f.stat().st_mode) == 0o600
    sauvegarde = tmp_path / "settings.json.bak-20260918-143005"
    assert stat.S_IMODE(sauvegarde.stat().st_mode) == 0o600


def test_modifier_settings_deux_sauvegardes_meme_seconde_ne_s_ecrasent_pas(tmp_path):
    f = tmp_path / "settings.json"
    f.write_text(json.dumps(settings_tiers()), encoding="utf-8")

    assert modifier_settings(f, ajouter_hooks, lambda d: True, horloge) == "modifié"
    apres_premier = json.loads(f.read_text())
    b0 = tmp_path / "settings.json.bak-20260918-143005"
    contenu_b0_apres_premier = b0.read_text()

    assert modifier_settings(f, retirer_hooks, lambda d: True, horloge) == "modifié"
    b1 = tmp_path / "settings.json.bak-20260918-143005-1"
    assert b1.exists()
    assert b0.read_text() == contenu_b0_apres_premier
    assert json.loads(b1.read_text()) == apres_premier


def test_modifier_settings_echec_a_l_ecriture_ne_laisse_pas_de_fichier_temporaire(tmp_path, monkeypatch):
    f = tmp_path / "settings.json"
    contenu_initial = json.dumps(settings_tiers())
    f.write_text(contenu_initial, encoding="utf-8")

    def replace_qui_echoue(*a, **kw):
        raise OSError("échec simulé")

    monkeypatch.setattr("discord_rich_claude.installation.os.replace", replace_qui_echoue)

    with pytest.raises(OSError):
        modifier_settings(f, ajouter_hooks, lambda d: True, horloge)

    assert f.read_text() == contenu_initial
    noms = {p.name for p in tmp_path.iterdir()}
    assert noms == {"settings.json", "settings.json.bak-20260918-143005"}


def test_modifier_settings_transformer_non_serialisable_ne_laisse_rien(tmp_path):
    f = tmp_path / "settings.json"
    contenu_initial = json.dumps({"model": "opus"})
    f.write_text(contenu_initial, encoding="utf-8")

    with pytest.raises(TypeError):
        modifier_settings(f, lambda d: {"objet": object()}, lambda d: True, horloge)

    assert f.read_text() == contenu_initial
    assert list(tmp_path.iterdir()) == [f]


# --- Fix round 1 : H3 -- formes qu'on ne sait pas modifier sans risque ---


def test_retirer_laisse_intacte_une_valeur_d_evenement_qui_est_un_objet():
    s = {"hooks": {"Stop": {"matcher": "x", "hooks": [dict(TIERS)]}}}
    assert retirer_hooks(s) == s


def test_retirer_laisse_intacte_une_valeur_d_evenement_qui_est_une_chaine():
    s = {"hooks": {"Stop": "abc"}}
    assert retirer_hooks(s) == s


def test_retirer_laisse_intact_un_groupe_qui_n_est_pas_un_objet():
    s = {"hooks": {"Stop": ["pas-un-objet"]}}
    assert retirer_hooks(s) == s


def test_retirer_laisse_intact_un_groupe_dont_les_hooks_ne_sont_pas_une_liste():
    s = {"hooks": {"Stop": [{"hooks": None}]}}
    assert retirer_hooks(s) == s


def test_retirer_garde_un_objet_hooks_deja_vide():
    assert retirer_hooks({"hooks": {}}) == {"hooks": {}}


def test_retirer_leve_si_hooks_n_est_pas_un_objet():
    with pytest.raises(ValueError):
        retirer_hooks({"hooks": "abc"})
    with pytest.raises(ValueError):
        retirer_hooks({"hooks": None})


def test_ajouter_leve_si_hooks_n_est_pas_un_objet():
    with pytest.raises(ValueError):
        ajouter_hooks({"hooks": "abc"})
    with pytest.raises(ValueError):
        ajouter_hooks({"hooks": None})


def test_ajouter_leve_si_un_evenement_n_est_pas_une_liste_et_nomme_l_evenement():
    with pytest.raises(ValueError, match="Stop"):
        ajouter_hooks({"hooks": {"Stop": "abc"}})


def test_ajouter_preserve_un_groupe_malforme_sans_planter():
    s = {"hooks": {"Stop": [{"hooks": None}]}}
    resultat = ajouter_hooks(s)
    groupes = resultat["hooks"]["Stop"]
    assert {"hooks": None} in groupes
    assert {"hooks": [{"type": "command", "command": COMMANDE_HOOK, "timeout": 2}]} in groupes


# --- LaunchAgent (Task 10) ---


def test_generer_plist(tmp_path):
    plist = plistlib.loads(generer_plist("/u/bin/discord-rich-claude", tmp_path))
    assert plist["Label"] == LABEL
    assert plist["ProgramArguments"] == ["/u/bin/discord-rich-claude", "run"]
    assert plist["RunAtLoad"] is True
    assert plist["KeepAlive"] is True
    assert plist["StandardErrorPath"] == str(tmp_path / "launchd.err.log")
    # K5 : ProcessType=Background throttlerait un process sur le chemin synchrone du hook.
    assert "ProcessType" not in plist


def test_installer_launch_agent(tmp_path):
    appels = []
    chemin = tmp_path / "LaunchAgents" / f"{LABEL}.plist"
    installer_launch_agent(chemin, b"<plist/>", executer=lambda cmd, **kw: appels.append(cmd))
    domaine = f"gui/{os.getuid()}"
    assert chemin.read_bytes() == b"<plist/>"
    assert appels == [
        ["launchctl", "bootout", f"{domaine}/{LABEL}"],
        ["launchctl", "bootstrap", domaine, str(chemin)],
    ]


def test_desinstaller_launch_agent(tmp_path):
    appels = []
    chemin = tmp_path / f"{LABEL}.plist"
    chemin.write_bytes(b"<plist/>")
    desinstaller_launch_agent(chemin, executer=lambda cmd, **kw: appels.append(cmd))
    assert not chemin.exists()
    assert appels == [["launchctl", "bootout", f"gui/{os.getuid()}/{LABEL}"]]
    desinstaller_launch_agent(chemin, executer=lambda cmd, **kw: None)  # déjà absent : pas d'erreur


# --- Fix round 1 (hardening avant l'installation réelle, Task 11) ---


# --- K3 : le plist est écrit (atomiquement) avant le bootout ---


def test_installer_launch_agent_ecrit_le_plist_avant_le_bootout(tmp_path):
    chemin = tmp_path / "LaunchAgents" / f"{LABEL}.plist"

    def verifie_deja_ecrit(cmd, **kw):
        assert chemin.read_bytes() == b"<plist/>"

    installer_launch_agent(chemin, b"<plist/>", executer=verifie_deja_ecrit)


def test_installer_launch_agent_echec_ecriture_n_appelle_pas_launchctl(tmp_path):
    appels = []
    dossier = tmp_path / "LaunchAgents"
    dossier.mkdir()
    chemin = dossier / f"{LABEL}.plist"
    os.chmod(dossier, 0o500)  # lecture/exécution seules : impossible d'y créer un fichier
    try:
        with pytest.raises(OSError):
            installer_launch_agent(chemin, b"<plist/>", executer=lambda cmd, **kw: appels.append(cmd))
    finally:
        os.chmod(dossier, 0o700)
    assert appels == []
    assert not chemin.exists()


# --- K4 : l'exécuteur par défaut n'est plus figé à la définition ---


def test_installer_launch_agent_executer_par_defaut_nest_pas_fige_a_la_definition():
    assert inspect.signature(installer_launch_agent).parameters["executer"].default is None


def test_desinstaller_launch_agent_executer_par_defaut_nest_pas_fige_a_la_definition():
    assert inspect.signature(desinstaller_launch_agent).parameters["executer"].default is None


# --- Fix round 2 (final-fix wave) : F6 -- réessai de bootstrap sur le code 5 ---


def _executeur_bootstrap_echoue_n_fois(n, code=5):
    appels = []

    def executer(cmd, **kw):
        appels.append(cmd)
        if cmd[1] == "bootstrap":
            deja = len([c for c in appels if c[1] == "bootstrap"])
            if deja <= n:
                raise subprocess.CalledProcessError(code, cmd)
        return subprocess.CompletedProcess(cmd, 0)

    return executer, appels


def test_installer_launch_agent_reessaie_bootstrap_sur_code_5_puis_reussit(tmp_path):
    chemin = tmp_path / "LaunchAgents" / f"{LABEL}.plist"
    executer, appels = _executeur_bootstrap_echoue_n_fois(2, code=5)
    dodo = []

    installer_launch_agent(chemin, b"<plist/>", executer=executer, dormir=dodo.append)

    bootstraps = [c for c in appels if c[1] == "bootstrap"]
    assert len(bootstraps) == 3
    assert dodo == [0.5, 0.5]


def test_installer_launch_agent_reessaie_bootstrap_sur_code_37(tmp_path):
    chemin = tmp_path / "LaunchAgents" / f"{LABEL}.plist"
    executer, appels = _executeur_bootstrap_echoue_n_fois(1, code=37)
    dodo = []

    installer_launch_agent(chemin, b"<plist/>", executer=executer, dormir=dodo.append)

    bootstraps = [c for c in appels if c[1] == "bootstrap"]
    assert len(bootstraps) == 2
    assert dodo == [0.5]


def test_installer_launch_agent_echoue_apres_3_tentatives_sur_code_5(tmp_path):
    chemin = tmp_path / "LaunchAgents" / f"{LABEL}.plist"
    executer, appels = _executeur_bootstrap_echoue_n_fois(4, code=5)
    dodo = []

    with pytest.raises(subprocess.CalledProcessError):
        installer_launch_agent(chemin, b"<plist/>", executer=executer, dormir=dodo.append)

    bootstraps = [c for c in appels if c[1] == "bootstrap"]
    assert len(bootstraps) == 3
    assert dodo == [0.5, 0.5]


def test_installer_launch_agent_autre_code_echoue_sans_reessai(tmp_path):
    chemin = tmp_path / "LaunchAgents" / f"{LABEL}.plist"
    executer, appels = _executeur_bootstrap_echoue_n_fois(4, code=1)
    dodo = []

    with pytest.raises(subprocess.CalledProcessError):
        installer_launch_agent(chemin, b"<plist/>", executer=executer, dormir=dodo.append)

    bootstraps = [c for c in appels if c[1] == "bootstrap"]
    assert len(bootstraps) == 1
    assert dodo == []


# --- Fix round 2 (final-fix wave) : F7 -- sauvegarde jamais plus large que son mode final ---


def test_modifier_settings_sauvegarde_deja_au_bon_mode_des_la_creation(tmp_path, monkeypatch):
    """F7 : si l'appel qui resserre les droits après coup est neutralisé, la
    sauvegarde doit déjà avoir le bon mode dès sa création (mode passé à la
    création elle-même, pas seulement un chmod après coup) — preuve qu'aucun
    instant plus permissif n'existe jamais entre la création et la correction,
    même sous un umask qui aurait élargi un mode par défaut (0600 original)."""
    f = tmp_path / "settings.json"
    f.write_text(json.dumps(settings_tiers()), encoding="utf-8")
    f.chmod(0o600)

    monkeypatch.setattr("discord_rich_claude.installation.os.chmod", lambda *a, **kw: None)
    monkeypatch.setattr("discord_rich_claude.installation.os.fchmod", lambda *a, **kw: None)
    ancien_umask = os.umask(0o022)
    try:
        assert modifier_settings(f, ajouter_hooks, lambda d: True, horloge) == "modifié"
    finally:
        os.umask(ancien_umask)

    sauvegarde = tmp_path / "settings.json.bak-20260918-143005"
    assert stat.S_IMODE(sauvegarde.stat().st_mode) == 0o600

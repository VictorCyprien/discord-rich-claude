import logging
import subprocess

import pytest

from discord_rich_claude import chemins, cli


def test_erreur_demarrage_os_error_retourne_1_logge_et_imprime_sans_trace(
    monkeypatch, tmp_path, caplog, capsys
):
    """G3 : sous le LaunchAgent (KeepAlive), une PermissionError non rattrapée au
    démarrage (dossier d'état invalide, cf. serveur._verifier_dossier_etat) donnerait
    une trace complète toutes les ~10 s dans un fichier non tourné. `lancer` doit la
    rattraper, logger une ligne (type + notre propre chemin de socket), l'imprimer
    brièvement sur stderr sans trace, et retourner 1."""
    cible = tmp_path / "ailleurs"
    cible.mkdir()
    dossier_etat = tmp_path / "state"
    dossier_etat.symlink_to(cible)  # dossier d'état invalide : un lien, pas un dossier

    monkeypatch.setattr(chemins, "SOCKET", dossier_etat / "daemon.sock")
    monkeypatch.setattr(chemins, "CONFIG", tmp_path / "config.toml")
    monkeypatch.setattr(chemins, "PROJETS_CLAUDE", tmp_path / "projects")
    monkeypatch.setattr(chemins, "DOSSIER_LOGS", tmp_path / "logs")

    racine = logging.getLogger()
    gestionnaires_avant = set(racine.handlers)
    niveau_avant = racine.level
    try:
        with caplog.at_level(logging.ERROR, logger="discord_rich_claude.cli"):
            code = cli.main(["run"])
    finally:
        for h in list(racine.handlers):
            if h not in gestionnaires_avant:
                racine.removeHandler(h)
                h.close()
        racine.setLevel(niveau_avant)

    assert code == 1

    erreurs = [r for r in caplog.records if r.levelno == logging.ERROR]
    assert len(erreurs) == 1
    assert "PermissionError" in erreurs[0].getMessage()
    assert str(dossier_etat / "daemon.sock") in erreurs[0].getMessage()

    sortie = capsys.readouterr()
    assert "Traceback" not in sortie.err
    assert "PermissionError" in sortie.err


# --- install, uninstall, status (Task 10) ---


def test_status_sans_daemon(monkeypatch, capsys, dossier_court):
    monkeypatch.setattr(cli.chemins, "SOCKET", dossier_court / "absent.sock")
    assert cli.main(["status"]) == 1
    assert "injoignable" in capsys.readouterr().out


def test_status_avec_daemon(monkeypatch, capsys):
    monkeypatch.setattr(cli, "interroger_etat", lambda chemin: {"discord": True, "sessions": 2})
    assert cli.main(["status"]) == 0
    sortie = capsys.readouterr().out
    assert "connecté" in sortie and "2" in sortie


def test_commande_inconnue():
    with pytest.raises(SystemExit):
        cli.main(["inconnue"])


def test_install_settings_invalide_imprime_sur_stderr_sans_trace_et_retourne_1(monkeypatch, capsys):
    """installation.modifier_settings peut lever ValueError (JSON invalide, forme de
    hooks non éditable, ou modification concurrente détectée à la relecture) : `install`
    doit l'annoncer sur stderr sans trace et ne rien avoir tenté d'autre ensuite."""

    def leve(*a, **kw):
        raise ValueError("settings.json a changé pendant la confirmation : relance la commande")

    monkeypatch.setattr(cli.os, "geteuid", lambda: 501)
    monkeypatch.setattr(cli, "modifier_settings", leve)
    assert cli.main(["install"]) == 1
    sortie = capsys.readouterr()
    assert "Traceback" not in sortie.err
    assert "relance la commande" in sortie.err
    assert "non modifié" in sortie.err


def test_uninstall_settings_invalide_imprime_sur_stderr_sans_trace_et_retourne_1(monkeypatch, capsys):
    def leve(*a, **kw):
        raise ValueError("settings['hooks'] doit être un objet")

    monkeypatch.setattr(cli.os, "geteuid", lambda: 501)
    monkeypatch.setattr(cli, "modifier_settings", leve)
    assert cli.main(["uninstall"]) == 1
    sortie = capsys.readouterr()
    assert "Traceback" not in sortie.err
    assert "doit être un objet" in sortie.err
    assert "non modifié" in sortie.err


# --- Fix round 1 (hardening avant l'installation réelle, Task 11) ---


# --- K1 : refus sous sudo ---


def test_install_refuse_sudo(monkeypatch, capsys):
    monkeypatch.setattr(cli.os, "geteuid", lambda: 0)

    def refuse_appel(*a, **kw):
        raise AssertionError("rien ne doit être tenté quand geteuid() == 0")

    monkeypatch.setattr(cli, "modifier_settings", refuse_appel)
    assert cli.main(["install"]) == 1
    sortie = capsys.readouterr()
    assert "sudo" in sortie.err
    assert "Traceback" not in sortie.err


def test_uninstall_refuse_sudo(monkeypatch, capsys):
    monkeypatch.setattr(cli.os, "geteuid", lambda: 0)

    def refuse_appel(*a, **kw):
        raise AssertionError("rien ne doit être tenté quand geteuid() == 0")

    monkeypatch.setattr(cli, "modifier_settings", refuse_appel)
    assert cli.main(["uninstall"]) == 1
    sortie = capsys.readouterr()
    assert "sudo" in sortie.err
    assert "Traceback" not in sortie.err


# --- K2 : exécutable absolu écrit dans le plist ---


def test_executable_absolu_prefere_argv0_absolu(tmp_path, monkeypatch):
    binaire = tmp_path / "discord-rich-claude"
    binaire.write_text("")
    monkeypatch.setattr(cli.sys, "argv", [str(binaire)])

    def which_ne_doit_pas_etre_appele(nom):
        raise AssertionError("which() ne doit pas être appelé si argv[0] convient")

    monkeypatch.setattr(cli.shutil, "which", which_ne_doit_pas_etre_appele)
    assert cli._executable_absolu() == str(binaire)


def test_executable_absolu_retombe_sur_which_si_argv0_relatif(monkeypatch):
    monkeypatch.setattr(cli.sys, "argv", ["discord-rich-claude"])
    monkeypatch.setattr(cli.shutil, "which", lambda nom: "/usr/local/bin/discord-rich-claude")
    assert cli._executable_absolu() == "/usr/local/bin/discord-rich-claude"


def test_executable_absolu_none_si_rien_d_absolu(monkeypatch):
    monkeypatch.setattr(cli.sys, "argv", ["discord-rich-claude"])
    monkeypatch.setattr(cli.shutil, "which", lambda nom: None)
    assert cli._executable_absolu() is None


def test_install_sans_executable_absolu_refuse_et_n_ecrit_rien(monkeypatch, capsys):
    monkeypatch.setattr(cli.os, "geteuid", lambda: 501)
    monkeypatch.setattr(cli, "modifier_settings", lambda *a, **kw: "inchangé")
    monkeypatch.setattr(cli, "creer_config_par_defaut", lambda chemin: True)
    monkeypatch.setattr(cli, "_executable_absolu", lambda: None)

    def refuse_appel(*a, **kw):
        raise AssertionError("installer_launch_agent ne doit pas être appelé sans exécutable absolu")

    monkeypatch.setattr(cli, "installer_launch_agent", refuse_appel)
    assert cli.main(["install"]) == 1
    sortie = capsys.readouterr()
    assert "Traceback" not in sortie.err


def test_install_imprime_l_executable_choisi(monkeypatch, capsys, tmp_path):
    monkeypatch.setattr(cli.os, "geteuid", lambda: 501)
    monkeypatch.setattr(cli, "modifier_settings", lambda *a, **kw: "inchangé")
    monkeypatch.setattr(cli, "creer_config_par_defaut", lambda chemin: True)
    monkeypatch.setattr(cli, "_executable_absolu", lambda: "/opt/bin/discord-rich-claude")
    monkeypatch.setattr(cli.chemins, "DOSSIER_LOGS", tmp_path / "logs")
    monkeypatch.setattr(cli, "installer_launch_agent", lambda *a, **kw: None)
    assert cli.main(["install"]) == 0
    sortie = capsys.readouterr().out
    assert "exécutable : /opt/bin/discord-rich-claude" in sortie


# --- K3 : pas de trace sur les erreurs réalistes ---


def test_confirmer_eof_est_un_refus(monkeypatch):
    def leve_eof(*a, **kw):
        raise EOFError

    monkeypatch.setattr("builtins.input", leve_eof)
    assert cli._confirmer("diff") is False


def test_confirmer_ctrl_c_est_un_refus(monkeypatch):
    def leve_interrupt(*a, **kw):
        raise KeyboardInterrupt

    monkeypatch.setattr("builtins.input", leve_interrupt)
    assert cli._confirmer("diff") is False


def test_install_config_echoue_stderr_sans_trace_et_retourne_1(monkeypatch, capsys):
    monkeypatch.setattr(cli.os, "geteuid", lambda: 501)
    monkeypatch.setattr(cli, "modifier_settings", lambda *a, **kw: "inchangé")

    def leve_permission(chemin):
        raise PermissionError("boom")

    monkeypatch.setattr(cli, "creer_config_par_defaut", leve_permission)

    def refuse_appel(*a, **kw):
        raise AssertionError("l'étape LaunchAgent ne doit pas être tentée")

    monkeypatch.setattr(cli, "installer_launch_agent", refuse_appel)
    assert cli.main(["install"]) == 1
    sortie = capsys.readouterr()
    assert "Traceback" not in sortie.err
    assert "PermissionError" in sortie.err


def test_install_ecriture_plist_echoue_stderr_sans_trace_et_retourne_1(monkeypatch, capsys, tmp_path):
    monkeypatch.setattr(cli.os, "geteuid", lambda: 501)
    monkeypatch.setattr(cli, "modifier_settings", lambda *a, **kw: "inchangé")
    monkeypatch.setattr(cli, "creer_config_par_defaut", lambda chemin: True)
    monkeypatch.setattr(cli, "_executable_absolu", lambda: "/opt/bin/discord-rich-claude")
    monkeypatch.setattr(cli.chemins, "DOSSIER_LOGS", tmp_path / "logs")

    def leve_permission(*a, **kw):
        raise PermissionError("boom")

    monkeypatch.setattr(cli, "installer_launch_agent", leve_permission)
    assert cli.main(["install"]) == 1
    sortie = capsys.readouterr()
    assert "Traceback" not in sortie.err
    assert "PermissionError" in sortie.err


def test_install_launchctl_echoue_message_contient_stderr(monkeypatch, capsys, tmp_path):
    monkeypatch.setattr(cli.os, "geteuid", lambda: 501)
    monkeypatch.setattr(cli, "modifier_settings", lambda *a, **kw: "inchangé")
    monkeypatch.setattr(cli, "creer_config_par_defaut", lambda chemin: True)
    monkeypatch.setattr(cli, "_executable_absolu", lambda: "/opt/bin/discord-rich-claude")
    monkeypatch.setattr(cli.chemins, "DOSSIER_LOGS", tmp_path / "logs")

    def leve_calledprocess(*a, **kw):
        raise subprocess.CalledProcessError(1, ["launchctl", "bootstrap"], stderr=b"boom")

    monkeypatch.setattr(cli, "installer_launch_agent", leve_calledprocess)
    assert cli.main(["install"]) == 1
    sortie = capsys.readouterr()
    assert "Traceback" not in sortie.err
    assert "boom" in sortie.err

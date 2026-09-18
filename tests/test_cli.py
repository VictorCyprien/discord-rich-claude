import logging

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

    monkeypatch.setattr(cli, "modifier_settings", leve)
    assert cli.main(["install"]) == 1
    sortie = capsys.readouterr()
    assert "Traceback" not in sortie.err
    assert "relance la commande" in sortie.err


def test_uninstall_settings_invalide_imprime_sur_stderr_sans_trace_et_retourne_1(monkeypatch, capsys):
    def leve(*a, **kw):
        raise ValueError("settings['hooks'] doit être un objet")

    monkeypatch.setattr(cli, "modifier_settings", leve)
    assert cli.main(["uninstall"]) == 1
    sortie = capsys.readouterr()
    assert "Traceback" not in sortie.err
    assert "doit être un objet" in sortie.err

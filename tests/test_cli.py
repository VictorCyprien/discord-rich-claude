import logging

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

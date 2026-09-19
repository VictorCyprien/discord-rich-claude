import os
import stat

import pytest

from discord_rich_claude.config import Config, ConfigSurveillee, charger, creer_config_par_defaut


def ecrire(chemin, texte):
    chemin.write_text(texte, encoding="utf-8")
    return chemin


def test_config_absente_refuse_tout(tmp_path):
    assert charger(tmp_path / "absente.toml") == Config()


def test_config_complete(tmp_path):
    projet = tmp_path / "projet"
    projet.mkdir()
    f = ecrire(
        tmp_path / "config.toml",
        f'client_id = "123"\ninactif_apres_min = 5\nrotation_s = 30\n'
        f'[projets]\nautorises = ["{projet}"]\n',
    )
    assert charger(f) == Config(
        client_id="123",
        inactif_apres_min=5,
        rotation_s=30,
        projets_autorises=(projet.resolve(),),
    )


def test_client_id_entier_accepte(tmp_path):
    f = ecrire(tmp_path / "c.toml", "client_id = 1234567890123456789\n")
    assert charger(f).client_id == "1234567890123456789"


def test_client_id_vide_vaut_absent(tmp_path):
    f = ecrire(tmp_path / "c.toml", 'client_id = ""\n')
    assert charger(f).client_id is None


def test_toml_invalide_refuse_tout(tmp_path):
    f = ecrire(tmp_path / "c.toml", "client_id = \n")
    assert charger(f) == Config()


def test_type_invalide_refuse_tout(tmp_path):
    f = ecrire(tmp_path / "c.toml", 'client_id = "1"\nrotation_s = "vite"\n')
    assert charger(f) == Config()


def test_booleen_refuse_comme_nombre(tmp_path):
    f = ecrire(tmp_path / "c.toml", "inactif_apres_min = true\n")
    assert charger(f) == Config()


def test_valeur_non_positive_refuse_tout(tmp_path):
    f = ecrire(tmp_path / "c.toml", "inactif_apres_min = 0\n")
    assert charger(f) == Config()


def test_chemin_relatif_refuse_tout(tmp_path):
    f = ecrire(tmp_path / "c.toml", '[projets]\nautorises = ["relatif/projet"]\n')
    assert charger(f) == Config()


def test_encodage_invalide_refuse_tout(tmp_path):
    f = tmp_path / "c.toml"
    f.write_bytes(b'client_id = "\xff\xfe bad"\n')
    assert charger(f) == Config()


def test_chemin_utilisateur_inconnu_refuse_tout(tmp_path):
    f = ecrire(
        tmp_path / "c.toml",
        '[projets]\nautorises = ["~nonexistentuser999/projet"]\n',
    )
    assert charger(f) == Config()


def test_valeur_non_finie_refuse_tout(tmp_path):
    f = ecrire(tmp_path / "c.toml", "inactif_apres_min = nan\n")
    assert charger(f) == Config()
    f = ecrire(tmp_path / "c2.toml", "rotation_s = inf\n")
    assert charger(f) == Config()


# --- Fix round 2 (final-fix wave) : F2 -- entier énorme et bornes ---


def test_entier_enorme_refuse_tout_sans_lever(tmp_path):
    f = ecrire(tmp_path / "c.toml", f"inactif_apres_min = {10 ** 309}\n")
    assert charger(f) == Config()


def test_rotation_s_enorme_refuse_tout_sans_lever(tmp_path):
    f = ecrire(tmp_path / "c.toml", f"rotation_s = {10 ** 309}\n")
    assert charger(f) == Config()


def test_rotation_s_minuscule_hors_bornes_refuse_tout(tmp_path):
    # 1e-320 passait la validation d'avant (positif), puis faisait lever
    # `est_vue_b` en boucle : hors bornes désormais.
    f = ecrire(tmp_path / "c.toml", "rotation_s = 1e-320\n")
    assert charger(f) == Config()


@pytest.mark.parametrize("valeur", [4, 3601])
def test_rotation_s_hors_bornes_5_3600(tmp_path, valeur):
    f = ecrire(tmp_path / "c.toml", f"rotation_s = {valeur}\n")
    assert charger(f) == Config()


@pytest.mark.parametrize("valeur", [5, 3600])
def test_rotation_s_aux_bornes_acceptee(tmp_path, valeur):
    f = ecrire(tmp_path / "c.toml", f"rotation_s = {valeur}\n")
    assert charger(f).rotation_s == valeur


@pytest.mark.parametrize("valeur", [0, 1441])
def test_inactif_apres_min_hors_bornes_1_1440(tmp_path, valeur):
    f = ecrire(tmp_path / "c.toml", f"inactif_apres_min = {valeur}\n")
    assert charger(f) == Config()


@pytest.mark.parametrize("valeur", [1, 1440])
def test_inactif_apres_min_aux_bornes_acceptee(tmp_path, valeur):
    f = ecrire(tmp_path / "c.toml", f"inactif_apres_min = {valeur}\n")
    assert charger(f).inactif_apres_min == valeur


def test_client_id_absent_garde_la_liste_blanche(tmp_path):
    projet = tmp_path / "p"
    projet.mkdir()
    f = ecrire(tmp_path / "c.toml", f'[projets]\nautorises = ["{projet}"]\n')
    c = charger(f)
    assert c.client_id is None
    assert c.projets_autorises == (projet.resolve(),)


def test_rechargement_seulement_si_le_fichier_change(tmp_path):
    f = ecrire(tmp_path / "c.toml", 'client_id = "1"\n')
    cs = ConfigSurveillee(f)
    assert cs.valeur.client_id == "1"
    assert cs.recharger_si_change() is False
    ecrire(f, 'client_id = "2"\n')
    stat = f.stat()
    os.utime(f, ns=(stat.st_atime_ns, stat.st_mtime_ns + 1_000_000_000))
    assert cs.recharger_si_change() is True
    assert cs.valeur.client_id == "2"


def test_rechargement_quand_le_fichier_disparait(tmp_path):
    f = ecrire(tmp_path / "c.toml", 'client_id = "1"\n')
    cs = ConfigSurveillee(f)
    f.unlink()
    assert cs.recharger_si_change() is True
    assert cs.valeur == Config()


# --- Config par défaut (Task 10) ---


def test_config_par_defaut_creee_et_refuse_tout(tmp_path):
    f = tmp_path / "sous" / "config.toml"
    assert creer_config_par_defaut(f) is True
    assert charger(f) == Config()


def test_config_par_defaut_n_ecrase_pas(tmp_path):
    f = tmp_path / "config.toml"
    f.write_text('client_id = "1"\n', encoding="utf-8")
    assert creer_config_par_defaut(f) is False
    assert f.read_text(encoding="utf-8") == 'client_id = "1"\n'


# --- Fix round 1 (hardening avant l'installation réelle, Task 11) : K6 ---


def test_config_par_defaut_creee_en_0600(tmp_path):
    f = tmp_path / "config.toml"
    assert creer_config_par_defaut(f) is True
    assert stat.S_IMODE(f.stat().st_mode) == 0o600


def test_config_par_defaut_lien_symbolique_pendant_refuse_sans_toucher_la_cible(tmp_path):
    cible = tmp_path / "cible-absente.toml"
    lien = tmp_path / "config.toml"
    lien.symlink_to(cible)
    assert creer_config_par_defaut(lien) is False
    assert not cible.exists()


# --- Task 12 : masquer_apres_min, masquage de la carte après une longue inactivité ---


def test_masquer_apres_min_defaut(tmp_path):
    assert charger(tmp_path / "absente.toml").masquer_apres_min == 60


def test_masquer_apres_min_valeur_valide_lue(tmp_path):
    f = ecrire(tmp_path / "c.toml", "masquer_apres_min = 30\n")
    assert charger(f).masquer_apres_min == 30


@pytest.mark.parametrize("valeur", [0, 1441])
def test_masquer_apres_min_hors_bornes_1_1440(tmp_path, valeur):
    f = ecrire(tmp_path / "c.toml", f"masquer_apres_min = {valeur}\n")
    assert charger(f) == Config()


@pytest.mark.parametrize("valeur", [1, 1440])
def test_masquer_apres_min_aux_bornes_acceptee(tmp_path, valeur):
    f = ecrire(tmp_path / "c.toml", f"masquer_apres_min = {valeur}\n")
    assert charger(f).masquer_apres_min == valeur


def test_masquer_apres_min_chaine_refuse_tout(tmp_path):
    f = ecrire(tmp_path / "c.toml", 'masquer_apres_min = "60"\n')
    assert charger(f) == Config()


def test_masquer_apres_min_booleen_refuse_tout(tmp_path):
    f = ecrire(tmp_path / "c.toml", "masquer_apres_min = true\n")
    assert charger(f) == Config()


def test_masquer_apres_min_non_fini_refuse_tout(tmp_path):
    f = ecrire(tmp_path / "c.toml", "masquer_apres_min = nan\n")
    assert charger(f) == Config()


def test_masquer_apres_min_enorme_refuse_tout_sans_lever(tmp_path):
    f = ecrire(tmp_path / "c.toml", f"masquer_apres_min = {10 ** 400}\n")
    assert charger(f) == Config()

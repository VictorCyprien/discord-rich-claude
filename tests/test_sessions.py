import pytest

from discord_rich_claude.sessions import (
    ATTEND_REPONSE,
    ATTEND_VALIDATION,
    INACTIF,
    REFLECHIT,
    TRAVAILLE,
    Evenement,
    Registre,
    activite_affichee,
    pid_vivant,
)

INACTIF_S = 600


def ev(nom, t, sid="s1", **kw):
    return Evenement(nom=nom, session_id=sid, recu_a=t, **kw)


def registre_demarre():
    r = Registre()
    r.appliquer(ev("SessionStart", 0))
    return r


def test_session_start_cree_une_session_qui_attend():
    r = Registre()
    r.appliquer(ev("SessionStart", 100, cwd="/p", transcript_path="/t.jsonl", pid=42))
    s = r.sessions["s1"]
    assert (s.debut, s.cwd, s.transcript_path, s.pid) == (100, "/p", "/t.jsonl", 42)
    assert activite_affichee(s, 100, INACTIF_S) == ATTEND_REPONSE


@pytest.mark.parametrize(
    "nom,outil,attendu",
    [
        ("UserPromptSubmit", None, REFLECHIT),
        ("PreToolUse", "Edit", "Écrit du code"),
        ("PreToolUse", "Write", "Écrit du code"),
        ("PreToolUse", "NotebookEdit", "Écrit du code"),
        ("PreToolUse", "Bash", "Lance une commande"),
        ("PreToolUse", "Read", "Lit le code"),
        ("PreToolUse", "Grep", "Lit le code"),
        ("PreToolUse", "Glob", "Lit le code"),
        ("PreToolUse", "WebSearch", "Cherche sur le web"),
        ("PreToolUse", "WebFetch", "Cherche sur le web"),
        ("PreToolUse", "Agent", "Pilote des sous-agents"),
        ("PreToolUse", "mcp__plane__list_issues", TRAVAILLE),
        ("PreToolUse", None, TRAVAILLE),
        ("PermissionRequest", "Bash", ATTEND_VALIDATION),
        ("Stop", None, ATTEND_REPONSE),
    ],
)
def test_evenements_du_fil_principal(nom, outil, attendu):
    r = registre_demarre()
    r.appliquer(ev(nom, 1, tool_name=outil))
    assert activite_affichee(r.sessions["s1"], 1, INACTIF_S) == attendu


def test_session_end_supprime_la_session():
    r = registre_demarre()
    r.appliquer(ev("SessionEnd", 1))
    assert r.sessions == {}


def test_session_end_inconnue_ne_cree_rien():
    r = Registre()
    r.appliquer(ev("SessionEnd", 1))
    assert r.sessions == {}


def test_session_inconnue_creee_au_premier_evenement():
    r = Registre()
    r.appliquer(ev("PreToolUse", 50, tool_name="Bash"))
    s = r.sessions["s1"]
    assert s.debut == 50
    assert activite_affichee(s, 50, INACTIF_S) == "Lance une commande"


def test_session_start_sur_session_existante_garde_le_debut():
    r = registre_demarre()
    r.appliquer(ev("SessionStart", 500))  # compaction, reprise
    assert r.sessions["s1"].debut == 0


def test_sous_agents_prioritaires_puis_retour_au_fil_principal():
    r = registre_demarre()
    r.appliquer(ev("SubagentStart", 1, agent_id="a1"))
    r.appliquer(ev("SubagentStart", 2, agent_id="a2"))
    r.appliquer(ev("Stop", 3))  # arrière-plan : le fil principal a fini son tour
    s = r.sessions["s1"]
    assert activite_affichee(s, 3, INACTIF_S) == "Pilote 2 sous-agents"
    r.appliquer(ev("SubagentStop", 4, agent_id="a1"))
    assert activite_affichee(s, 4, INACTIF_S) == "Pilote 1 sous-agent"
    r.appliquer(ev("SubagentStop", 5, agent_id="a2"))
    assert activite_affichee(s, 5, INACTIF_S) == ATTEND_REPONSE


def test_evenement_de_sous_agent_ne_touche_pas_le_fil_principal():
    r = Registre()
    r.appliquer(ev("SessionStart", 0, cwd="/p", transcript_path="/t.jsonl"))
    r.appliquer(ev("Stop", 1))
    r.appliquer(
        ev("PreToolUse", 2, agent_id="a1", tool_name="Edit", cwd="/autre", transcript_path="/a.jsonl")
    )
    s = r.sessions["s1"]
    assert s.activite == ATTEND_REPONSE
    assert (s.cwd, s.transcript_path) == ("/p", "/t.jsonl")
    assert "a1" in s.sous_agents  # reconstruit sans SubagentStart


def test_inactif_prioritaire_sur_les_sous_agents():
    r = Registre()
    r.appliquer(ev("SubagentStart", 0, agent_id="a1"))
    s = r.sessions["s1"]
    assert activite_affichee(s, 599, INACTIF_S) == "Pilote 1 sous-agent"
    assert activite_affichee(s, 600, INACTIF_S) == INACTIF


def test_evenement_de_sous_agent_repousse_l_inactivite():
    r = registre_demarre()
    r.appliquer(ev("Stop", 1))
    r.appliquer(ev("PreToolUse", 500, agent_id="a1", tool_name="Read"))
    assert activite_affichee(r.sessions["s1"], 1000, INACTIF_S) != INACTIF


def test_purge_des_sessions_au_pid_mort():
    r = Registre()
    r.appliquer(ev("SessionStart", 0, pid=42))
    r.appliquer(ev("SessionStart", 0, sid="s2", pid=43))
    r.purger(10, INACTIF_S, pid_vivant=lambda pid: pid == 43)
    assert list(r.sessions) == ["s2"]


def test_purge_garde_une_session_sans_pid():
    r = registre_demarre()
    r.purger(10_000, INACTIF_S, pid_vivant=lambda pid: False)
    assert "s1" in r.sessions


def test_purge_retire_les_sous_agents_muets():
    r = registre_demarre()
    r.appliquer(ev("SubagentStart", 0, agent_id="a1"))
    r.appliquer(ev("SubagentStart", 500, agent_id="a2"))
    r.purger(650, INACTIF_S, pid_vivant=lambda pid: True)
    assert list(r.sessions["s1"].sous_agents) == ["a2"]


def test_session_affichee_et_debut_le_plus_ancien():
    r = Registre()
    r.appliquer(ev("SessionStart", 0, sid="s1"))
    r.appliquer(ev("SessionStart", 10, sid="s2"))
    r.appliquer(ev("Stop", 20, sid="s1"))
    assert r.session_affichee().session_id == "s1"
    assert r.debut_plus_ancien() == 0


def test_registre_vide():
    r = Registre()
    assert r.session_affichee() is None
    assert r.debut_plus_ancien() is None


def test_pid_vivant():
    import os

    assert pid_vivant(os.getpid())
    assert not pid_vivant(2**22 + 12345)  # au-delà des PID macOS

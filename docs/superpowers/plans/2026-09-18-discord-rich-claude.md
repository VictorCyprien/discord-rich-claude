# Discord Rich Presence pour Claude Code — plan d'implémentation

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Un daemon Python local, alimenté par les hooks de Claude Code, qui affiche sur le profil Discord une carte « Joue à Claude Code » (activité, modèle, tokens, durée), avec le nom du projet seulement pour les projets en liste blanche.

**Architecture:** Chaque hook Claude Code est une ligne `curl` synchrone qui poste le JSON du hook sur un socket Unix. Le daemon (asyncio) filtre les champs, tient l'état des sessions en mémoire, lit les transcripts pour les tokens, construit la carte par une fonction pure et la publie via pypresence avec un débit limité. Un LaunchAgent le démarre avec la session macOS.

**Tech Stack:** Python ≥ 3.12 (3.13 en local), uv, asyncio, pypresence 4.6.x, pytest 9 + pytest-asyncio 1.4, hatchling.

**Spec:** `docs/superpowers/specs/2026-09-18-discord-rich-claude-design.md`. Lire la spec avant chaque tâche : le plan argumente depuis elle.

## Global Constraints

- Python `>=3.12`. Dépendance d'exécution unique : `pypresence>=4.6.2,<5`. Tout le reste en stdlib.
- Aucun accès réseau : ni télémétrie, ni vérification de mise à jour. Seule la connexion IPC locale à Discord.
- Champs lus d'un événement, liste fermée : `session_id`, `cwd`, `hook_event_name`, `tool_name`, `agent_id`, `transcript_path`, plus l'en-tête `X-Claude-Pid`. Le reste est jeté à la réception, jamais gardé ni écrit dans un log.
- Jamais sur Discord : noms de fichiers, commandes, prompts, branche git, chemins. Nom de projet seulement via la liste blanche (chemins complets résolus).
- Libellés affichés en français, codés en dur, exactement ceux de la spec (sections 4.2, 4.3, 5).
- Tokens comptés : `input_tokens + cache_creation_input_tokens + output_tokens`. `cache_read_input_tokens` exclu.
- Chemins : socket `~/.local/state/discord-rich-claude/daemon.sock` (dossier 0700, socket 0600) ; config `~/.config/discord-rich-claude/config.toml` ; logs `~/Library/Logs/discord-rich-claude/` ; LaunchAgent `~/Library/LaunchAgents/local.discord-rich-claude.plist`, label `local.discord-rich-claude`.
- Commande de hook, exacte : `curl -s --max-time 1 --unix-socket "$HOME/.local/state/discord-rich-claude/daemon.sock" -H "X-Claude-Pid: $PPID" --data-binary @- http://localhost/event >/dev/null 2>&1 || true`, hook synchrone, `timeout: 2`.
- Logs via `logging`, jamais `print` dans le daemon (les `print` de la CLI interactive sont permis). Niveau INFO par défaut, DEBUG avec `DRC_LOG=DEBUG`.
- TDD : test qui échoue, code, test qui passe, commit. Un commit par tâche au minimum, Conventional Commits, message terminé par la ligne `Co-Authored-By` en vigueur dans la session.
- Git : une branche `feature/lN-<nom>` par lot, partie de `develop`, mergée dans `develop` quand la preuve du lot est faite. Pas de remote, pas de push.
- Routage des sous-agents : implémentation en `sonnet`, relecture sécurité et cohérence en `opus`.
- Lancer les tests : `uv run pytest -q` à la racine du dépôt.

## Lots

Le découpage raffine celui de la spec (section 11) : les modules purs, tokens compris, sont regroupés en L2 pour que la carte affichée en L3 soit complète dès sa première apparition.

| Lot | Tâches | Preuve |
|---|---|---|
| L1 — sonde | 1 | Section 12 de la spec remplie avec les résultats ; arrêt si une hypothèse tombe |
| L2 — logique | 2, 3, 4, 5 | `uv run pytest -q` vert, nombre de tests exécutés rapporté |
| L3 — daemon vivant | 6, 7, 8 | Test bout en bout vert + carte vue sur le profil Discord pendant une vraie session |
| L4 — installation | 9, 10, 11 | Installation réelle sur `~/.claude/settings.json`, carte présente après redémarrage de la session macOS, protocole manuel passé |

## Structure des fichiers

```
pyproject.toml                       # Task 2
.gitignore                           # Task 1
README.md, CLAUDE.md                 # Task 11
sonde/sonde.py                       # Task 1 — hook de sonde L1
src/discord_rich_claude/
  __init__.py                        # Task 2
  chemins.py                         # Task 2 — chemins et commande de hook
  sessions.py                        # Task 2 — état des sessions, pur
  config.py                          # Task 3 (+ Task 10)
  carte.py                           # Task 4 — carte Discord, pur
  tokens.py                          # Task 5 — lecture des transcripts
  presence.py                        # Task 6 — client Discord, débit, reconnexion
  serveur.py                         # Task 7 — socket Unix, HTTP minimal, filtrage
  daemon.py                          # Task 8 — assemblage, boucles
  cli.py                             # Task 8 (run) puis Task 10 (install, uninstall, status)
  installation.py                    # Task 9 (settings.json) + Task 10 (LaunchAgent)
tests/
  conftest.py                        # Task 7 — dossier court pour les sockets
  test_sessions.py, test_config.py, test_carte.py, test_tokens.py,
  test_presence.py, test_serveur.py, test_daemon.py, test_installation.py, test_cli.py
```

---

## Lot L1 — sonde

### Task 1: Sonde des hooks et vérification des hypothèses

But : vérifier les hypothèses de la section 12 de la spec avant d'écrire le daemon. La sonde journalise des métadonnées de hooks, jamais leur contenu.

**Files:**
- Create: `.gitignore`
- Create: `sonde/sonde.py`
- Create (non commité) : `.claude/settings.local.json`
- Modify: `docs/superpowers/specs/2026-09-18-discord-rich-claude-design.md` (section 12)

**Interfaces:**
- Consumes : rien.
- Produces : section 12 de la spec remplie. Valeur confirmée de l'intervalle minimal Discord, utilisée en Task 6 (`INTERVALLE_MIN_S`).

- [ ] **Step 1: Créer la branche du lot**

```bash
git checkout develop && git checkout -b feature/l1-sonde
```

- [ ] **Step 2: Écrire `.gitignore`**

```gitignore
.venv/
__pycache__/
*.pyc
.pytest_cache/
dist/
.claude/settings.local.json
sonde/sonde.log
```

- [ ] **Step 3: Écrire `sonde/sonde.py`**

```python
#!/usr/bin/env python3
"""Sonde du lot L1 : journalise les métadonnées des hooks Claude Code.

Ne garde jamais le contenu (prompt, tool_input, messages) : seulement les noms
de clés reçues et quelques champs d'identification.
"""
import json
import subprocess
import sys
import time
from pathlib import Path

JOURNAL = Path(__file__).with_name("sonde.log")
GARDES = (
    "hook_event_name", "session_id", "agent_id", "agent_type", "tool_name",
    "transcript_path", "agent_transcript_path", "cwd", "permission_mode",
    "source", "reason",
)


def main() -> None:
    brut = json.load(sys.stdin)
    ppid = sys.argv[1] if len(sys.argv) > 1 else ""
    comm = ""
    if ppid.isascii() and ppid.isdigit():
        comm = subprocess.run(
            ["ps", "-o", "comm=", "-p", ppid], capture_output=True, text=True
        ).stdout.strip()
    ligne = {
        "t": round(time.time(), 3),
        "ppid": ppid,
        "ppid_comm": comm,
        "cles": sorted(brut),
        **{k: brut[k] for k in GARDES if k in brut},
    }
    with JOURNAL.open("a", encoding="utf-8") as f:
        f.write(json.dumps(ligne, ensure_ascii=False) + "\n")


if __name__ == "__main__":
    try:
        main()
    except Exception:  # une sonde ne doit jamais gêner Claude Code
        pass
```

- [ ] **Step 4: Vérifier que la sonde ne garde pas le contenu**

Run:
```bash
echo '{"hook_event_name":"Stop","session_id":"x","prompt":"SECRET-123"}' | python3 sonde/sonde.py 4242
cat sonde/sonde.log; grep -c SECRET-123 sonde/sonde.log; rm sonde/sonde.log
```
Expected : une ligne JSON avec `"ppid": "4242"` et `"prompt"` dans `cles` ; `grep -c` affiche `0`.

- [ ] **Step 5: Brancher la sonde sur ce projet seulement**

Vérifier d'abord que `.claude/settings.local.json` n'existe pas (`ls .claude/settings.local.json`). S'il existe : arrêter et demander à Victor. Sinon, créer `.claude/settings.local.json` :

```json
{
  "hooks": {
    "SessionStart": [{"hooks": [{"type": "command", "command": "python3 \"$CLAUDE_PROJECT_DIR/sonde/sonde.py\" \"$PPID\"", "timeout": 5}]}],
    "UserPromptSubmit": [{"hooks": [{"type": "command", "command": "python3 \"$CLAUDE_PROJECT_DIR/sonde/sonde.py\" \"$PPID\"", "timeout": 5}]}],
    "PreToolUse": [{"matcher": "*", "hooks": [{"type": "command", "command": "python3 \"$CLAUDE_PROJECT_DIR/sonde/sonde.py\" \"$PPID\"", "timeout": 5}]}],
    "PermissionRequest": [{"hooks": [{"type": "command", "command": "python3 \"$CLAUDE_PROJECT_DIR/sonde/sonde.py\" \"$PPID\"", "timeout": 5}]}],
    "Stop": [{"hooks": [{"type": "command", "command": "python3 \"$CLAUDE_PROJECT_DIR/sonde/sonde.py\" \"$PPID\"", "timeout": 5}]}],
    "SubagentStart": [{"hooks": [{"type": "command", "command": "python3 \"$CLAUDE_PROJECT_DIR/sonde/sonde.py\" \"$PPID\"", "timeout": 5}]}],
    "SubagentStop": [{"hooks": [{"type": "command", "command": "python3 \"$CLAUDE_PROJECT_DIR/sonde/sonde.py\" \"$PPID\"", "timeout": 5}]}],
    "SessionEnd": [{"hooks": [{"type": "command", "command": "python3 \"$CLAUDE_PROJECT_DIR/sonde/sonde.py\" \"$PPID\"", "timeout": 5}]}]
  }
}
```

- [ ] **Step 6: Commit**

```bash
git add .gitignore sonde/sonde.py
git commit -m "chore: add hook probe for L1 assumptions"
```

- [ ] **Step 7: CHECKPOINT MANUEL — Victor lance la session de sonde**

Arrêter l'exécution et donner à Victor ces instructions, puis attendre qu'il confirme :

1. Dans un nouveau terminal : `cd /Volumes/MAC-Extended/Workspace-MAC/DiscordRichClaude && claude`
2. Premier prompt, à coller tel quel : « Lance deux sous-agents Explore en arrière-plan qui listent chacun le contenu de docs/. Puis lance un sous-agent Explore au premier plan qui lit docs/superpowers/specs. Attends la fin des deux sous-agents en arrière-plan avant de répondre. »
3. Deuxième prompt : « Exécute la commande `mkdir -p /tmp/sonde-permission && rmdir /tmp/sonde-permission` ». Dans le mode habituel (auto) : noter si une demande de validation apparaît. Si aucune n'apparaît, passer en mode Manual (shift+tab) et redemander la même commande, puis valider.
4. `/exit`, puis donner le résultat de `date`.

- [ ] **Step 8: Analyser le journal**

Run :
```bash
python3 - <<'EOF'
import json, subprocess
L = [json.loads(l) for l in open("sonde/sonde.log", encoding="utf-8")]
principales = {l["session_id"] for l in L if "agent_id" not in l}
print("sessions du fil principal :", principales)
print("H1 ppid_comm :", sorted({(l["ppid"], l["ppid_comm"]) for l in L}))
for ppid in {l["ppid"] for l in L}:
    vivant = subprocess.run(["ps", "-p", ppid], capture_output=True).returncode == 0
    print(f"H1 ppid {ppid} encore vivant après /exit : {vivant}")
sa = [l for l in L if "agent_id" in l]
print("H2 session_id des événements de sous-agents :", sorted({l["session_id"] for l in sa}))
print("H2 égal au fil principal :", {l["session_id"] for l in sa} <= principales)
for nom in ("SubagentStart", "SubagentStop"):
    ev = [l for l in L if l["hook_event_name"] == nom]
    print(f"H3 {nom} : {len(ev)} ; agent_id présents : {[l.get('agent_id') for l in ev]} ; clés : {ev[0]['cles'] if ev else None}")
print("H4 PermissionRequest :", [(l.get("tool_name"), l.get("permission_mode")) for l in L if l["hook_event_name"] == "PermissionRequest"])
print("outil de lancement de sous-agent :", sorted({l.get("tool_name") for l in L if l["hook_event_name"] == "PreToolUse" and "agent_id" not in l}))
print("H7 transcripts :", sorted({(l.get("transcript_path"), l.get("agent_transcript_path")) for l in L if "agent_id" in l or l["hook_event_name"] == "SubagentStop"}))
EOF
ls "$(dirname "$(python3 -c 'import json;print([json.loads(l) for l in open("sonde/sonde.log")][0]["transcript_path"])')")"/*/subagents/ 2>/dev/null | head
```
Expected : de quoi trancher H1, H2, H3, H4, H7, et le nom exact de l'outil qui lance un sous-agent (supposé `Agent`).

- [ ] **Step 9: H6, limite de fréquence de Discord**

Chercher dans la documentation Discord (docs.discord.com, Rich Presence / Social SDK) la limite de mise à jour d'activité. Valeur connue au moment du plan : « one update per 15 seconds » (docs.discord.com, Setting Rich Presence) ; l'issue discord/discord-api-docs#668 montre que dépasser cette limite peut vider la présence au lieu de mettre en file. Retenir la valeur documentée la plus stricte trouvée. `INTERVALLE_MIN_S` de la Task 6 vaut 15.0 par défaut ; ne le changer que si une source plus récente donne une autre valeur.

- [ ] **Step 10: Consigner les résultats dans la spec**

Dans la section 12 de la spec, ajouter sous chaque hypothèse une ligne `Résultat (2026-09-xx) : confirmée / infirmée — <preuve en une phrase>`. H5 reste « à mesurer en Task 8 ». Ajouter le nom réel de l'outil de sous-agent s'il n'est pas `Agent`.

**Si une hypothèse est infirmée : arrêter ici.** Rapporter à Victor l'hypothèse, la preuve et la section de la spec touchée (H1 → 4.4, repli de purge à 12 h ; H2 ou H3 → 4.1 et 4.3 ; H4 → 4.1 ; H7 → 6). La spec et ce plan se révisent avant la Task 2.

- [ ] **Step 11: Débrancher la sonde, commit, merge**

```bash
rm .claude/settings.local.json sonde/sonde.log
git add docs/superpowers/specs/2026-09-18-discord-rich-claude-design.md
git commit -m "docs: record L1 probe results in spec"
git checkout develop && git merge --no-ff feature/l1-sonde -m "merge: L1 hook probe"
```

---

## Lot L2 — logique

### Task 2: Squelette du projet, chemins et état des sessions

**Files:**
- Create: `pyproject.toml`
- Create: `src/discord_rich_claude/__init__.py`
- Create: `src/discord_rich_claude/chemins.py`
- Create: `src/discord_rich_claude/sessions.py`
- Test: `tests/test_sessions.py`

**Interfaces:**
- Consumes : rien.
- Produces :
  - `chemins` : constantes `DOSSIER_ETAT`, `SOCKET`, `CONFIG`, `DOSSIER_LOGS`, `PROJETS_CLAUDE`, `SETTINGS_CLAUDE`, `LAUNCH_AGENT`, `LABEL`, `COMMANDE_HOOK` (toutes `Path` sauf `LABEL` et `COMMANDE_HOOK`, `str`).
  - `sessions.Evenement(nom: str, session_id: str, recu_a: float, cwd: str | None = None, tool_name: str | None = None, agent_id: str | None = None, transcript_path: str | None = None, pid: int | None = None)`, dataclass figée.
  - `sessions.Session` (dataclass mutable) : `session_id`, `debut: float`, `dernier_evenement: float`, `activite: str`, `cwd`, `transcript_path`, `pid`, `sous_agents: dict[str, float]`.
  - `sessions.Registre` : attribut `sessions: dict[str, Session]` ; méthodes `appliquer(ev: Evenement) -> None`, `purger(maintenant: float, inactif_apres_s: float, pid_vivant: Callable[[int], bool]) -> None`, `session_affichee() -> Session | None`, `debut_plus_ancien() -> float | None`.
  - `sessions.activite_affichee(s: Session, maintenant: float, inactif_apres_s: float) -> str`, `sessions.libelle_outil(tool_name: str | None) -> str`, `sessions.pid_vivant(pid: int) -> bool`.
  - Constantes de libellés : `REFLECHIT`, `ATTEND_REPONSE`, `ATTEND_VALIDATION`, `INACTIF`, `TRAVAILLE`.

- [ ] **Step 1: Créer la branche du lot**

```bash
git checkout develop && git checkout -b feature/l2-logique
```

- [ ] **Step 2: Écrire `pyproject.toml`**

```toml
[project]
name = "discord-rich-claude"
version = "0.1.0"
description = "Discord Rich Presence pour Claude Code, pilotée par les hooks"
requires-python = ">=3.12"
dependencies = ["pypresence>=4.6.2,<5"]

[project.scripts]
discord-rich-claude = "discord_rich_claude.cli:main"

[dependency-groups]
dev = ["pytest>=9", "pytest-asyncio>=1.4"]

[build-system]
requires = ["hatchling"]
build-backend = "hatchling.build"

[tool.hatch.build.targets.wheel]
packages = ["src/discord_rich_claude"]

[tool.pytest.ini_options]
asyncio_mode = "auto"
testpaths = ["tests"]
```

- [ ] **Step 3: Écrire `src/discord_rich_claude/__init__.py` et `chemins.py`**

`__init__.py` :
```python
"""Discord Rich Presence pour Claude Code."""
```

`chemins.py` :
```python
"""Emplacements sur disque et commande de hook. Voir la spec, sections 7.5 et 8."""
from pathlib import Path

HOME = Path.home()
DOSSIER_ETAT = HOME / ".local" / "state" / "discord-rich-claude"
SOCKET = DOSSIER_ETAT / "daemon.sock"
CONFIG = HOME / ".config" / "discord-rich-claude" / "config.toml"
DOSSIER_LOGS = HOME / "Library" / "Logs" / "discord-rich-claude"
PROJETS_CLAUDE = HOME / ".claude" / "projects"
SETTINGS_CLAUDE = HOME / ".claude" / "settings.json"
LABEL = "local.discord-rich-claude"
LAUNCH_AGENT = HOME / "Library" / "LaunchAgents" / f"{LABEL}.plist"

# $HOME et $PPID sont développés par le shell du hook : $PPID y désigne le
# processus Claude Code, que le daemon surveille pour purger les sessions mortes.
COMMANDE_HOOK = (
    'curl -s --max-time 1 --unix-socket "$HOME/.local/state/discord-rich-claude/daemon.sock" '
    '-H "X-Claude-Pid: $PPID" --data-binary @- http://localhost/event >/dev/null 2>&1 || true'
)
```

- [ ] **Step 4: Écrire les tests qui échouent, `tests/test_sessions.py`**

```python
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
```

- [ ] **Step 5: Vérifier que les tests échouent**

Run: `uv sync && uv run pytest tests/test_sessions.py -q`
Expected : FAIL, `ModuleNotFoundError: No module named 'discord_rich_claude.sessions'`.

- [ ] **Step 6: Écrire `src/discord_rich_claude/sessions.py`**

```python
"""État des sessions Claude Code, en mémoire. Logique pure, sans entrée-sortie.

Voir la spec, section 4.
"""
import os
from collections.abc import Callable
from dataclasses import dataclass, field

REFLECHIT = "Réfléchit"
ATTEND_REPONSE = "Attend ta réponse"
ATTEND_VALIDATION = "Attend ta validation"
INACTIF = "Inactif"
TRAVAILLE = "Travaille"

LIBELLES_OUTILS = {
    "Edit": "Écrit du code",
    "Write": "Écrit du code",
    "NotebookEdit": "Écrit du code",
    "Bash": "Lance une commande",
    "Read": "Lit le code",
    "Grep": "Lit le code",
    "Glob": "Lit le code",
    "WebSearch": "Cherche sur le web",
    "WebFetch": "Cherche sur le web",
    "Agent": "Pilote des sous-agents",
}


@dataclass(frozen=True)
class Evenement:
    nom: str
    session_id: str
    recu_a: float
    cwd: str | None = None
    tool_name: str | None = None
    agent_id: str | None = None
    transcript_path: str | None = None
    pid: int | None = None


@dataclass
class Session:
    session_id: str
    debut: float
    dernier_evenement: float
    activite: str = TRAVAILLE
    cwd: str | None = None
    transcript_path: str | None = None
    pid: int | None = None
    sous_agents: dict[str, float] = field(default_factory=dict)


def libelle_outil(tool_name: str | None) -> str:
    return LIBELLES_OUTILS.get(tool_name or "", TRAVAILLE)


def activite_affichee(s: Session, maintenant: float, inactif_apres_s: float) -> str:
    if maintenant - s.dernier_evenement >= inactif_apres_s:
        return INACTIF
    n = len(s.sous_agents)
    if n == 1:
        return "Pilote 1 sous-agent"
    if n > 1:
        return f"Pilote {n} sous-agents"
    return s.activite


def pid_vivant(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:  # le processus existe mais appartient à un autre compte
        return True
    return True


class Registre:
    def __init__(self) -> None:
        self.sessions: dict[str, Session] = {}

    def appliquer(self, ev: Evenement) -> None:
        if ev.nom == "SessionEnd":
            self.sessions.pop(ev.session_id, None)
            return
        s = self.sessions.get(ev.session_id)
        if s is None:
            s = Session(ev.session_id, debut=ev.recu_a, dernier_evenement=ev.recu_a)
            self.sessions[ev.session_id] = s
        s.dernier_evenement = ev.recu_a
        if ev.pid is not None:
            s.pid = ev.pid

        if ev.agent_id is not None:
            # Un sous-agent compte comme activité mais ne change pas le fil principal.
            if ev.nom == "SubagentStop":
                s.sous_agents.pop(ev.agent_id, None)
            else:
                s.sous_agents[ev.agent_id] = ev.recu_a
            return

        if ev.cwd:
            s.cwd = ev.cwd
        if ev.transcript_path:
            s.transcript_path = ev.transcript_path
        if ev.nom in ("SessionStart", "Stop"):
            s.activite = ATTEND_REPONSE
        elif ev.nom == "UserPromptSubmit":
            s.activite = REFLECHIT
        elif ev.nom == "PreToolUse":
            s.activite = libelle_outil(ev.tool_name)
        elif ev.nom == "PermissionRequest":
            s.activite = ATTEND_VALIDATION

    def purger(
        self, maintenant: float, inactif_apres_s: float, pid_vivant: Callable[[int], bool]
    ) -> None:
        for sid, s in list(self.sessions.items()):
            if s.pid is not None and not pid_vivant(s.pid):
                del self.sessions[sid]
                continue
            s.sous_agents = {
                a: t for a, t in s.sous_agents.items() if maintenant - t < inactif_apres_s
            }

    def session_affichee(self) -> Session | None:
        return max(self.sessions.values(), key=lambda s: s.dernier_evenement, default=None)

    def debut_plus_ancien(self) -> float | None:
        return min((s.debut for s in self.sessions.values()), default=None)
```

Note : `SubagentStart` et `SubagentStop` portent l'`agent_id` du sous-agent (vérifié en Task 1, H3). Si la Task 1 a montré un autre nom de champ ou un autre nom d'outil que `Agent`, la spec a été révisée : suivre la spec.

- [ ] **Step 7: Vérifier que les tests passent**

Run: `uv run pytest tests/test_sessions.py -q`
Expected : PASS, 30 tests.

- [ ] **Step 8: Commit**

```bash
git add pyproject.toml uv.lock src/discord_rich_claude/__init__.py src/discord_rich_claude/chemins.py src/discord_rich_claude/sessions.py tests/test_sessions.py
git commit -m "feat: add session state registry"
```

---

### Task 3: Configuration

**Files:**
- Create: `src/discord_rich_claude/config.py`
- Test: `tests/test_config.py`

**Interfaces:**
- Consumes : rien.
- Produces :
  - `config.Config(client_id: str | None = None, inactif_apres_min: float = 10, rotation_s: float = 20, projets_autorises: tuple[Path, ...] = ())`, dataclass figée.
  - `config.charger(chemin: Path) -> Config` : ne lève jamais ; fichier absent, illisible ou invalide → `Config()` (refus par défaut) et un avertissement dans le log.
  - `config.ConfigSurveillee(chemin: Path)` : attribut `valeur: Config`, méthode `recharger_si_change() -> bool`.

- [ ] **Step 1: Écrire les tests qui échouent, `tests/test_config.py`**

```python
import os

from discord_rich_claude.config import Config, ConfigSurveillee, charger


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
```

- [ ] **Step 2: Vérifier que les tests échouent**

Run: `uv run pytest tests/test_config.py -q`
Expected : FAIL, `ModuleNotFoundError: No module named 'discord_rich_claude.config'`.

- [ ] **Step 3: Écrire `src/discord_rich_claude/config.py`**

```python
"""Lecture de config.toml. Voir la spec, section 7.1.

Tout défaut de config retombe sur Config() : aucun projet autorisé, pas de
client_id, donc rien de montré (refus par défaut).
"""
import logging
import tomllib
from dataclasses import dataclass
from pathlib import Path

log = logging.getLogger(__name__)


@dataclass(frozen=True)
class Config:
    client_id: str | None = None
    inactif_apres_min: float = 10
    rotation_s: float = 20
    projets_autorises: tuple[Path, ...] = ()


def charger(chemin: Path) -> Config:
    try:
        with chemin.open("rb") as f:
            brut = tomllib.load(f)
    except FileNotFoundError:
        log.warning("config absente : %s", chemin)
        return Config()
    except (OSError, tomllib.TOMLDecodeError) as e:
        log.warning("config illisible, rien ne sera affiché : %s", e)
        return Config()
    try:
        return _valider(brut)
    except (TypeError, ValueError) as e:
        log.warning("config invalide, rien ne sera affiché : %s", e)
        return Config()


def _valider(brut: dict) -> Config:
    cid = brut.get("client_id")
    if cid is not None and (isinstance(cid, bool) or not isinstance(cid, (str, int))):
        raise TypeError("client_id doit être un texte")
    projets = brut.get("projets", {})
    if not isinstance(projets, dict):
        raise TypeError("[projets] doit être une table")
    autorises = projets.get("autorises", [])
    if not isinstance(autorises, list) or not all(isinstance(p, str) for p in autorises):
        raise TypeError("projets.autorises doit être une liste de chemins")
    chemins = []
    for p in autorises:
        chemin = Path(p).expanduser()
        if not chemin.is_absolute():
            raise ValueError(f"chemin relatif refusé : {p}")
        chemins.append(chemin.resolve())
    return Config(
        client_id=str(cid) if cid not in (None, "") else None,
        inactif_apres_min=_positif(brut.get("inactif_apres_min", 10), "inactif_apres_min"),
        rotation_s=_positif(brut.get("rotation_s", 20), "rotation_s"),
        projets_autorises=tuple(chemins),
    )


def _positif(valeur: object, nom: str) -> float:
    if isinstance(valeur, bool) or not isinstance(valeur, (int, float)):
        raise TypeError(f"{nom} doit être un nombre")
    if valeur <= 0:
        raise ValueError(f"{nom} doit être positif")
    return valeur


def _mtime(chemin: Path) -> int | None:
    try:
        return chemin.stat().st_mtime_ns
    except OSError:
        return None


class ConfigSurveillee:
    """Recharge la config quand la date de modification du fichier change."""

    def __init__(self, chemin: Path) -> None:
        self.chemin = chemin
        self._mtime = _mtime(chemin)
        self.valeur = charger(chemin)

    def recharger_si_change(self) -> bool:
        mtime = _mtime(self.chemin)
        if mtime == self._mtime:
            return False
        self._mtime = mtime
        self.valeur = charger(self.chemin)
        return True
```

- [ ] **Step 4: Vérifier que les tests passent**

Run: `uv run pytest tests/test_config.py -q`
Expected : PASS, 12 tests.

- [ ] **Step 5: Commit**

```bash
git add src/discord_rich_claude/config.py tests/test_config.py
git commit -m "feat: add toml config with deny-by-default fallback"
```

---

### Task 4: La carte Discord

**Files:**
- Create: `src/discord_rich_claude/carte.py`
- Test: `tests/test_carte.py`

**Interfaces:**
- Consumes : `sessions.Registre`, `sessions.activite_affichee` (Task 2) ; `config.Config` (Task 3).
- Produces :
  - `carte.Carte(details: str, state: str, debut: int, large_image: str = "logo", large_text: str = "Claude Code")`, dataclass figée, comparable par `==`.
  - `carte.Stats(modele: str | None = None, tokens_session: int = 0, tokens_jour: int = 0)`, dataclass figée.
  - `carte.construire_carte(registre: Registre, config: Config, stats: Stats, maintenant: float, vue_b: bool) -> Carte | None`.
  - `carte.est_vue_b(maintenant: float, rotation_s: float) -> bool`, `carte.formater_nombre(n: int) -> str`, `carte.nom_modele(model_id: str | None) -> str | None`, `carte.nom_projet(cwd: str | None, autorises: tuple[Path, ...]) -> str`.

- [ ] **Step 1: Écrire les tests qui échouent, `tests/test_carte.py`**

```python
import pytest

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
```

- [ ] **Step 2: Vérifier que les tests échouent**

Run: `uv run pytest tests/test_carte.py -q`
Expected : FAIL, `ModuleNotFoundError: No module named 'discord_rich_claude.carte'`.

- [ ] **Step 3: Écrire `src/discord_rich_claude/carte.py`**

```python
"""Construction de la carte Discord. Fonctions pures. Voir la spec, section 5."""
import re
from dataclasses import dataclass
from pathlib import Path

from .config import Config
from .sessions import Registre, activite_affichee

PROJET_PRIVE = "Projet privé"
_MODELE = re.compile(r"^claude-(opus|sonnet|haiku|fable)-(\d+)(?:-(\d{1,2}))?(?:-\d{8})?$")


@dataclass(frozen=True)
class Carte:
    details: str
    state: str
    debut: int
    large_image: str = "logo"
    large_text: str = "Claude Code"


@dataclass(frozen=True)
class Stats:
    modele: str | None = None
    tokens_session: int = 0
    tokens_jour: int = 0


def formater_nombre(n: int) -> str:
    if n < 1_000:
        return str(n)
    if n < 1_000_000:
        return f"{n // 1_000} k"
    return f"{n / 1_000_000:.1f} M".replace(".", ",")


def nom_modele(model_id: str | None) -> str | None:
    if not model_id:
        return None
    m = _MODELE.match(model_id)
    if m is None:
        return model_id.removeprefix("claude-")
    famille, majeur, mineur = m.groups()
    version = f"{majeur}.{mineur}" if mineur else majeur
    return f"{famille.capitalize()} {version}"


def nom_projet(cwd: str | None, autorises: tuple[Path, ...]) -> str:
    # Comparaison sur chemins complets résolus : un homonyme ou un dossier dont le
    # nom commence pareil n'est jamais autorisé par erreur.
    if not cwd:
        return PROJET_PRIVE
    chemin = Path(cwd).resolve()
    for autorise in autorises:
        if chemin == autorise or autorise in chemin.parents:
            return autorise.name
    return PROJET_PRIVE


def est_vue_b(maintenant: float, rotation_s: float) -> bool:
    return int(maintenant // rotation_s) % 2 == 1


def construire_carte(
    registre: Registre, config: Config, stats: Stats, maintenant: float, vue_b: bool
) -> Carte | None:
    session = registre.session_affichee()
    if session is None:
        return None
    activite = activite_affichee(session, maintenant, config.inactif_apres_min * 60)
    details = f"{activite} · {nom_projet(session.cwd, config.projets_autorises)}"
    if vue_b:
        n = len(registre.sessions)
        pluriel = "s" if n > 1 else ""
        state = f"{n} session{pluriel} · {formater_nombre(stats.tokens_jour)} aujourd'hui"
    else:
        tokens = f"{formater_nombre(stats.tokens_session)} tokens"
        modele = nom_modele(stats.modele)
        state = f"{modele} · {tokens}" if modele else tokens
    return Carte(details=details, state=state, debut=int(registre.debut_plus_ancien()))
```

- [ ] **Step 4: Vérifier que les tests passent**

Run: `uv run pytest tests/test_carte.py -q`
Expected : PASS, 29 tests.

- [ ] **Step 5: Commit**

```bash
git add src/discord_rich_claude/carte.py tests/test_carte.py
git commit -m "feat: build Discord card with project allowlist"
```

---

### Task 5: Tokens et modèle depuis les transcripts

**Files:**
- Create: `src/discord_rich_claude/tokens.py`
- Test: `tests/test_tokens.py`

**Interfaces:**
- Consumes : rien.
- Produces :
  - `tokens.SuiviTokens(racine: Path)` : `rafraichir(transcripts: Iterable[Path], minuit: float) -> None`, `tokens_session(transcript: Path) -> int`, `tokens_jour(minuit: float) -> int`, `modele(transcript: Path) -> str | None`, propriété `fichiers_suivis -> frozenset[Path]`.
  - `tokens.minuit_local(maintenant: float) -> float`, `tokens.fichiers_de_session(transcript: Path) -> list[Path]`.
  - Seules les méthodes de `SuiviTokens` touchent le disque ; `SuiviTokens` n'est pas thread-safe (un seul appelant à la fois, garanti par le daemon en Task 8).

- [ ] **Step 1: Écrire les tests qui échouent, `tests/test_tokens.py`**

```python
import json
import os
import time
from datetime import datetime, timedelta

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
```

- [ ] **Step 2: Vérifier que les tests échouent**

Run: `uv run pytest tests/test_tokens.py -q`
Expected : FAIL, `ModuleNotFoundError: No module named 'discord_rich_claude.tokens'`.

- [ ] **Step 3: Écrire `src/discord_rich_claude/tokens.py`**

```python
"""Tokens et modèle lus dans les transcripts JSONL de Claude Code. Voir la spec, section 6.

Les transcripts répètent une même réponse avec des compteurs différents (écriture
en flux) : on garde une entrée par (message.id, requestId), avec le maximum de
chaque champ. Garder la première occurrence sous-compte.
"""
import json
import logging
from collections.abc import Iterable
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path

log = logging.getLogger(__name__)

# Les lectures du cache sont exclues : elles grossissent avec la longueur du
# contexte, pas avec le travail fait.
CHAMPS_COMPTES = ("input_tokens", "cache_creation_input_tokens", "output_tokens")

Cle = tuple[str, str]


@dataclass
class Entree:
    horodatage: float | None
    tokens: dict[str, int]


def minuit_local(maintenant: float) -> float:
    d = datetime.fromtimestamp(maintenant).astimezone()
    return d.replace(hour=0, minute=0, second=0, microsecond=0).timestamp()


def fichiers_de_session(transcript: Path) -> list[Path]:
    sous_agents = transcript.with_suffix("") / "subagents"
    return [transcript, *sorted(sous_agents.glob("*.jsonl"))]


def _horodatage(brut: object) -> float | None:
    if not isinstance(brut, str):
        return None
    try:
        return datetime.fromisoformat(brut).timestamp()
    except ValueError:
        return None


def extraire(obj: dict) -> tuple[Cle, float | None, dict[str, int]] | None:
    msg = obj.get("message")
    if not isinstance(msg, dict) or not isinstance(msg.get("usage"), dict):
        return None
    ident = msg.get("id") or obj.get("uuid")
    if not ident:
        return None
    usage = msg["usage"]
    try:
        tokens = {k: int(usage.get(k) or 0) for k in CHAMPS_COMPTES}
    except (TypeError, ValueError):
        return None
    cle = (str(ident), str(obj.get("requestId") or ""))
    return cle, _horodatage(obj.get("timestamp")), tokens


def modele_de(obj: dict) -> str | None:
    msg = obj.get("message")
    if obj.get("type") != "assistant" or not isinstance(msg, dict):
        return None
    modele = msg.get("model")
    if isinstance(modele, str) and modele and modele != "<synthetic>":
        return modele
    return None


class Fichier:
    """Un transcript lu par morceaux : seules les nouvelles lignes complètes."""

    def __init__(self, chemin: Path) -> None:
        self.chemin = chemin
        self.decalage = 0
        self.entrees: dict[Cle, Entree] = {}
        self.modele: str | None = None

    def rafraichir(self) -> None:
        for obj in self._nouvelles_lignes():
            extrait = extraire(obj)
            if extrait is not None:
                cle, horodatage, tokens = extrait
                entree = self.entrees.get(cle)
                if entree is None:
                    self.entrees[cle] = Entree(horodatage, tokens)
                else:
                    entree.tokens = {k: max(v, entree.tokens.get(k, 0)) for k, v in tokens.items()}
            modele = modele_de(obj)
            if modele is not None:
                self.modele = modele

    def _nouvelles_lignes(self) -> list[dict]:
        try:
            if self.chemin.stat().st_size < self.decalage:
                # Fichier réécrit plus court : on repart de zéro.
                self.decalage, self.entrees, self.modele = 0, {}, None
            with self.chemin.open("rb") as f:
                f.seek(self.decalage)
                bloc = f.read()
        except FileNotFoundError:
            return []
        fin = bloc.rfind(b"\n")
        if fin < 0:
            return []
        self.decalage += fin + 1
        objets = []
        for brute in bloc[:fin].split(b"\n"):
            if not brute.strip():
                continue
            try:
                obj = json.loads(brute)
            except ValueError:
                log.warning("ligne illisible ignorée dans %s", self.chemin.name)
                continue
            if isinstance(obj, dict):
                objets.append(obj)
        return objets


def _total(fichiers: Iterable[Fichier], depuis: float | None = None) -> int:
    fusion: dict[Cle, dict[str, int]] = {}
    for fichier in fichiers:
        for cle, entree in fichier.entrees.items():
            if depuis is not None and (entree.horodatage is None or entree.horodatage < depuis):
                continue
            courant = fusion.setdefault(cle, {})
            for k, v in entree.tokens.items():
                courant[k] = max(courant.get(k, 0), v)
    return sum(sum(t.values()) for t in fusion.values())


class SuiviTokens:
    def __init__(self, racine: Path) -> None:
        self.racine = racine
        self._fichiers: dict[Path, Fichier] = {}

    @property
    def fichiers_suivis(self) -> frozenset[Path]:
        return frozenset(self._fichiers)

    def rafraichir(self, transcripts: Iterable[Path], minuit: float) -> None:
        voulus: set[Path] = set()
        for transcript in transcripts:
            voulus.update(fichiers_de_session(transcript))
        for chemin in self.racine.glob("**/*.jsonl"):
            try:
                if chemin.stat().st_mtime >= minuit:
                    voulus.add(chemin)
            except FileNotFoundError:
                pass
        for chemin in list(self._fichiers):
            if chemin not in voulus:
                del self._fichiers[chemin]
        for chemin in voulus:
            self._fichiers.setdefault(chemin, Fichier(chemin)).rafraichir()

    def tokens_session(self, transcript: Path) -> int:
        return _total(
            self._fichiers[p] for p in fichiers_de_session(transcript) if p in self._fichiers
        )

    def tokens_jour(self, minuit: float) -> int:
        return _total(self._fichiers.values(), depuis=minuit)

    def modele(self, transcript: Path) -> str | None:
        fichier = self._fichiers.get(transcript)
        return fichier.modele if fichier else None
```

- [ ] **Step 4: Vérifier que les tests passent**

Run: `uv run pytest tests/test_tokens.py -q`
Expected : PASS, 12 tests.

- [ ] **Step 5: Vérifier sur un vrai transcript**

Recompter indépendamment la session de brainstorming du 2026-09-18 et comparer :

```bash
uv run python - <<'EOF'
from pathlib import Path
from discord_rich_claude.tokens import SuiviTokens, minuit_local
racine = Path.home() / ".claude/projects/-Volumes-MAC-Extended-Workspace-MAC-DiscordRichClaude"
t = racine / "01cce4fe-2f6a-45fa-ac8c-fd27229880d9.jsonl"
s = SuiviTokens(racine)
s.rafraichir([t], minuit_local(0))
print("session :", s.tokens_session(t), "modèle :", s.modele(t))
EOF
```
Expected : environ 445 k au moment du brainstorming, plus élevé depuis car la session a continué (mesure indépendante faite ce jour-là : 445 439 sans les lectures du cache). Le modèle vaut `claude-opus-5`. Un écart d'un ordre de grandeur signale un bug de dédoublonnage.

- [ ] **Step 6: Lancer toute la suite, commit, merge du lot**

Run: `uv run pytest -q`
Expected : PASS, 83 tests (30 + 12 + 29 + 12). Rapporter le nombre réellement exécuté.

```bash
git add src/discord_rich_claude/tokens.py tests/test_tokens.py
git commit -m "feat: count session and daily tokens from transcripts"
git checkout develop && git merge --no-ff feature/l2-logique -m "merge: L2 pure logic"
```

---

## Lot L3 — daemon vivant

### Task 6: Connexion Discord, débit et reconnexion

**Files:**
- Create: `src/discord_rich_claude/presence.py`
- Test: `tests/test_presence.py`

**Interfaces:**
- Consumes : `carte.Carte` (Task 4).
- Produces :
  - `presence.ClientDiscord` (Protocol) : `async connecter()`, `async publier(carte: Carte)`, `async effacer()`, `async fermer()`.
  - `presence.ClientPypresence(client_id: str)` : implémentation réelle.
  - `presence.Presence(fabrique: Callable[[str], ClientDiscord], intervalle_min_s: float = INTERVALLE_MIN_S, reessai_s: float = REESSAI_S, horloge: Callable[[], float] = time.monotonic)` : `async soumettre(carte: Carte | None, client_id: str | None) -> None`, `async fermer() -> None`, propriété `connecte -> bool`.
  - Constantes `INTERVALLE_MIN_S = 15.0`, `REESSAI_S = 15.0`.

- [ ] **Step 1: Créer la branche du lot**

```bash
git checkout develop && git checkout -b feature/l3-daemon
```

- [ ] **Step 2: Écrire les tests qui échouent, `tests/test_presence.py`**

```python
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
```

- [ ] **Step 3: Vérifier que les tests échouent**

Run: `uv run pytest tests/test_presence.py -q`
Expected : FAIL, `ModuleNotFoundError: No module named 'discord_rich_claude.presence'`.

- [ ] **Step 4: Écrire `src/discord_rich_claude/presence.py`**

```python
"""Connexion à Discord : publication à débit limité, reconnexion. Voir la spec, sections 4.5, 5 et 9."""
import logging
import math
import time
from collections.abc import Callable
from typing import Protocol

from pypresence import AioPresence

from .carte import Carte

log = logging.getLogger(__name__)

# Discord documente « one update per 15 seconds » ; au-delà, la présence peut
# être vidée au lieu d'être mise en file (discord-api-docs#668). Valeur confirmée en Task 1.
INTERVALLE_MIN_S = 15.0
REESSAI_S = 15.0


class ClientDiscord(Protocol):
    async def connecter(self) -> None: ...
    async def publier(self, carte: Carte) -> None: ...
    async def effacer(self) -> None: ...
    async def fermer(self) -> None: ...


class ClientPypresence:
    def __init__(self, client_id: str) -> None:
        self._rpc = AioPresence(client_id, connection_timeout=5, response_timeout=5)

    async def connecter(self) -> None:
        await self._rpc.connect()

    async def publier(self, carte: Carte) -> None:
        await self._rpc.update(
            details=carte.details,
            state=carte.state,
            start=carte.debut,
            large_image=carte.large_image,
            large_text=carte.large_text,
        )

    async def effacer(self) -> None:
        await self._rpc.clear()

    async def fermer(self) -> None:
        self._rpc.close()


class Presence:
    def __init__(
        self,
        fabrique: Callable[[str], ClientDiscord],
        intervalle_min_s: float = INTERVALLE_MIN_S,
        reessai_s: float = REESSAI_S,
        horloge: Callable[[], float] = time.monotonic,
    ) -> None:
        self._fabrique = fabrique
        self._intervalle_min_s = intervalle_min_s
        self._reessai_s = reessai_s
        self._horloge = horloge
        self._client: ClientDiscord | None = None
        self._client_id: str | None = None
        self._derniere: Carte | None = None
        self._dernier_envoi = -math.inf
        self._dernier_essai = -math.inf

    @property
    def connecte(self) -> bool:
        return self._client is not None

    async def soumettre(self, carte: Carte | None, client_id: str | None) -> None:
        if carte is None or client_id is None or client_id != self._client_id:
            await self.fermer()
        if carte is None or client_id is None:
            return
        maintenant = self._horloge()
        if self._client is None:
            if maintenant - self._dernier_essai < self._reessai_s:
                return
            self._dernier_essai = maintenant
            try:
                client = self._fabrique(client_id)
                await client.connecter()
            except Exception as e:
                log.info("Discord injoignable, nouvel essai dans %.0f s : %s", self._reessai_s, e)
                return
            self._client, self._client_id = client, client_id
            log.info("connecté à Discord")
        if carte == self._derniere or maintenant - self._dernier_envoi < self._intervalle_min_s:
            return
        try:
            await self._client.publier(carte)
        except Exception as e:
            log.warning("publication refusée, reconnexion dans %.0f s : %s", self._reessai_s, e)
            client, self._client, self._client_id, self._derniere = self._client, None, None, None
            self._dernier_essai = maintenant
            try:
                await client.fermer()
            except Exception:
                pass
            return
        self._derniere, self._dernier_envoi = carte, maintenant

    async def fermer(self) -> None:
        client, self._client, self._client_id = self._client, None, None
        self._derniere, self._dernier_envoi = None, -math.inf
        if client is None:
            return
        for action in (client.effacer, client.fermer):
            try:
                await action()
            except Exception as e:
                log.debug("fermeture de la connexion Discord : %s", e)
```

- [ ] **Step 5: Vérifier que les tests passent**

Run: `uv run pytest tests/test_presence.py -q`
Expected : PASS, 8 tests.

- [ ] **Step 6: Commit**

```bash
git add src/discord_rich_claude/presence.py tests/test_presence.py
git commit -m "feat: publish Discord presence with rate limit and reconnect"
```

---

### Task 7: Serveur sur socket Unix et filtrage des événements

**Files:**
- Create: `src/discord_rich_claude/serveur.py`
- Create: `tests/conftest.py`
- Test: `tests/test_serveur.py`

**Interfaces:**
- Consumes : `sessions.Evenement` (Task 2).
- Produces :
  - `serveur.filtrer(corps: bytes, pid_brut: str | None, recu_a: float, projets_claude: Path) -> Evenement | None`.
  - `serveur.transcript_autorise(chemin: str, racine: Path) -> bool`.
  - `serveur.demarrer_serveur(chemin_socket: Path, sur_evenement: Callable[[Evenement], None], etat: Callable[[], dict], projets_claude: Path) -> asyncio.Server` (coroutine). Lève `serveur.DejaLance` si un daemon écoute déjà.
  - `serveur.interroger_etat(chemin_socket: Path, delai_s: float = 1.0) -> dict | None` (synchrone, pour `status`).
  - Routes : `POST /event` → 204 ; `GET /etat` → 200 JSON de `etat()` ; autre → 404.
  - Fixture de test `dossier_court` (conftest) : dossier sous `/tmp`, car un chemin de socket macOS est limité à 104 octets.

- [ ] **Step 1: Écrire `tests/conftest.py`**

```python
import shutil
import tempfile
from pathlib import Path

import pytest


@pytest.fixture
def dossier_court():
    """Dossier court sous /tmp : sous macOS, un chemin de socket Unix est limité à 104 octets."""
    d = Path(tempfile.mkdtemp(prefix="drc-", dir="/tmp"))
    yield d
    shutil.rmtree(d, ignore_errors=True)
```

- [ ] **Step 2: Écrire les tests qui échouent, `tests/test_serveur.py`**

```python
import asyncio
import json
import stat

import pytest

from discord_rich_claude.serveur import (
    DejaLance,
    demarrer_serveur,
    filtrer,
    interroger_etat,
    transcript_autorise,
)


def corps(**champs):
    return json.dumps(champs).encode()


def test_filtrer_ne_garde_que_la_liste_fermee(tmp_path):
    ev = filtrer(
        corps(
            hook_event_name="PreToolUse",
            session_id="abc-123",
            cwd="/p",
            tool_name="Write",
            tool_input={"file_path": "/secret/fichier.py", "content": "SECRET-CONTENU"},
            prompt="SECRET-PROMPT",
            last_assistant_message="SECRET-REPONSE",
        ),
        "4242",
        10.0,
        tmp_path,
    )
    assert (ev.nom, ev.session_id, ev.cwd, ev.tool_name, ev.pid, ev.recu_a) == (
        "PreToolUse", "abc-123", "/p", "Write", 4242, 10.0,
    )
    assert "SECRET" not in repr(ev)


@pytest.mark.parametrize(
    "brut",
    [
        b"{pas du json",
        b"\xff\xfe",
        b"[1, 2]",
        corps(hook_event_name="Notification", session_id="s1"),
        corps(hook_event_name="Stop"),
        corps(hook_event_name="Stop", session_id=""),
        corps(hook_event_name="Stop", session_id="../../etc"),
        corps(hook_event_name="Stop", session_id="s1", tool_name=3),
    ],
)
def test_filtrer_refuse(brut, tmp_path):
    assert filtrer(brut, None, 0.0, tmp_path) is None


@pytest.mark.parametrize(
    "pid_brut,attendu", [("123", 123), (None, None), ("abc", None), ("1", None), ("0", None), ("１２", None)]
)
def test_filtrer_pid(pid_brut, attendu, tmp_path):
    assert filtrer(corps(hook_event_name="Stop", session_id="s1"), pid_brut, 0.0, tmp_path).pid == attendu


def test_transcript_autorise(tmp_path):
    racine = tmp_path / "projects"
    (racine / "p").mkdir(parents=True)
    assert transcript_autorise(str(racine / "p" / "s1.jsonl"), racine)
    assert not transcript_autorise(str(racine / "p" / "s1.txt"), racine)
    assert not transcript_autorise(str(tmp_path / "ailleurs.jsonl"), racine)
    assert not transcript_autorise(str(racine / ".." / "ailleurs.jsonl"), racine)
    assert not transcript_autorise("relatif/s1.jsonl", racine)


def test_filtrer_efface_un_transcript_hors_racine(tmp_path):
    ev = filtrer(
        corps(hook_event_name="Stop", session_id="s1", transcript_path="/etc/passwd.jsonl"),
        None,
        0.0,
        tmp_path,
    )
    assert ev.transcript_path is None


async def requete(socket_, brute: bytes) -> bytes:
    lecteur, ecrivain = await asyncio.open_unix_connection(str(socket_))
    ecrivain.write(brute)
    await ecrivain.drain()
    reponse = await lecteur.read()
    ecrivain.close()
    return reponse


def post(corps_: bytes, entetes: str = "") -> bytes:
    return (
        f"POST /event HTTP/1.1\r\nHost: localhost\r\nContent-Length: {len(corps_)}\r\n{entetes}\r\n"
    ).encode() + corps_


@pytest.fixture
async def serveur_test(dossier_court, tmp_path):
    recus = []
    socket_ = dossier_court / "etat" / "daemon.sock"
    serveur = await demarrer_serveur(socket_, recus.append, lambda: {"sessions": 2}, tmp_path)
    yield socket_, recus
    serveur.close()
    await serveur.wait_closed()


async def test_post_event(serveur_test):
    socket_, recus = serveur_test
    reponse = await requete(socket_, post(corps(hook_event_name="Stop", session_id="s1"), "X-Claude-Pid: 77\r\n"))
    assert reponse.startswith(b"HTTP/1.1 204")
    assert [(e.nom, e.pid) for e in recus] == [("Stop", 77)]


async def test_expect_100_continue(serveur_test):
    socket_, recus = serveur_test
    reponse = await requete(socket_, post(corps(hook_event_name="Stop", session_id="s1"), "Expect: 100-continue\r\n"))
    assert reponse.startswith(b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 204")
    assert len(recus) == 1


async def test_corps_trop_gros_refuse(serveur_test):
    socket_, recus = serveur_test
    brute = b"POST /event HTTP/1.1\r\nContent-Length: 17000000\r\n\r\n"
    assert (await requete(socket_, brute)).startswith(b"HTTP/1.1 413")
    assert recus == []


async def test_sans_content_length(serveur_test):
    socket_, _ = serveur_test
    assert (await requete(socket_, b"POST /event HTTP/1.1\r\n\r\n")).startswith(b"HTTP/1.1 411")


async def test_route_inconnue(serveur_test):
    socket_, _ = serveur_test
    assert (await requete(socket_, b"GET / HTTP/1.1\r\n\r\n")).startswith(b"HTTP/1.1 404")


async def test_etat(serveur_test):
    socket_, _ = serveur_test
    assert await asyncio.to_thread(interroger_etat, socket_) == {"sessions": 2}


def test_etat_sans_daemon(dossier_court):
    assert interroger_etat(dossier_court / "absent.sock") is None


async def test_permissions(serveur_test):
    socket_, _ = serveur_test
    assert stat.S_IMODE(socket_.stat().st_mode) == 0o600
    assert stat.S_IMODE(socket_.parent.stat().st_mode) == 0o700


async def test_deuxieme_instance_refusee(serveur_test, tmp_path):
    socket_, _ = serveur_test
    with pytest.raises(DejaLance):
        await demarrer_serveur(socket_, lambda e: None, dict, tmp_path)


async def test_socket_orphelin_remplace(dossier_court, tmp_path):
    socket_ = dossier_court / "orphelin.sock"
    socket_.touch()
    serveur = await demarrer_serveur(socket_, lambda e: None, dict, tmp_path)
    serveur.close()
    await serveur.wait_closed()


async def test_erreur_du_gestionnaire_n_arrete_pas_le_serveur(dossier_court, tmp_path):
    socket_ = dossier_court / "d.sock"
    appels = []

    def gestionnaire(ev):
        appels.append(ev)
        if len(appels) == 1:
            raise RuntimeError("boum")

    serveur = await demarrer_serveur(socket_, gestionnaire, dict, tmp_path)
    brute = post(corps(hook_event_name="Stop", session_id="s1"))
    assert (await requete(socket_, brute)).startswith(b"HTTP/1.1 500")
    assert (await requete(socket_, brute)).startswith(b"HTTP/1.1 204")
    serveur.close()
    await serveur.wait_closed()
```

- [ ] **Step 3: Vérifier que les tests échouent**

Run: `uv run pytest tests/test_serveur.py -q`
Expected : FAIL, `ModuleNotFoundError: No module named 'discord_rich_claude.serveur'`.

- [ ] **Step 4: Écrire `src/discord_rich_claude/serveur.py`**

```python
"""Réception des hooks sur un socket Unix. Voir la spec, sections 3, 7.3 et 7.5.

HTTP minimal codé à la main : une route d'écriture, une de lecture, rien d'autre.
Le JSON d'un hook contient le prompt et les entrées d'outil : filtrer() n'en
garde qu'une liste fermée de champs, le reste n'est jamais conservé.
"""
import asyncio
import functools
import json
import logging
import os
import re
import socket
import time
from collections.abc import Callable
from contextlib import suppress
from pathlib import Path

from .sessions import Evenement

log = logging.getLogger(__name__)

TAILLE_MAX = 16 * 1024 * 1024  # un PreToolUse sur Write transporte tout le fichier
DELAI_S = 2.0
CHAMPS_LUS = ("session_id", "cwd", "hook_event_name", "tool_name", "agent_id", "transcript_path")
EVENEMENTS = frozenset(
    {
        "SessionStart", "UserPromptSubmit", "PreToolUse", "PermissionRequest",
        "Stop", "SubagentStart", "SubagentStop", "SessionEnd",
    }
)
SESSION_ID = re.compile(r"^[A-Za-z0-9_-]{1,128}$")
RAISONS = {
    200: "OK", 204: "No Content", 404: "Not Found", 411: "Length Required",
    413: "Content Too Large", 500: "Internal Server Error",
}


class DejaLance(RuntimeError):
    """Un autre daemon écoute déjà sur le socket."""


def transcript_autorise(chemin: str, racine: Path) -> bool:
    p = Path(chemin)
    if not p.is_absolute() or p.suffix != ".jsonl":
        return False
    return racine.resolve() in p.resolve().parents


def _pid(brut: str | None) -> int | None:
    if brut and brut.isascii() and brut.isdigit():
        pid = int(brut)
        return pid if pid > 1 else None
    return None


def filtrer(corps: bytes, pid_brut: str | None, recu_a: float, projets_claude: Path) -> Evenement | None:
    try:
        brut = json.loads(corps)
    except ValueError:
        return None
    if not isinstance(brut, dict):
        return None
    champs = {k: brut.get(k) for k in CHAMPS_LUS}
    del brut
    if any(v is not None and not isinstance(v, str) for v in champs.values()):
        return None
    nom, session_id = champs["hook_event_name"], champs["session_id"]
    if nom not in EVENEMENTS or not session_id or not SESSION_ID.match(session_id):
        return None
    transcript = champs["transcript_path"]
    if transcript is not None and not transcript_autorise(transcript, projets_claude):
        transcript = None
    return Evenement(
        nom=nom,
        session_id=session_id,
        recu_a=recu_a,
        cwd=champs["cwd"],
        tool_name=champs["tool_name"],
        agent_id=champs["agent_id"],
        transcript_path=transcript,
        pid=_pid(pid_brut),
    )


async def _repondre(ecrivain: asyncio.StreamWriter, code: int, corps: bytes = b"") -> None:
    tete = f"HTTP/1.1 {code} {RAISONS[code]}\r\nContent-Length: {len(corps)}\r\nConnection: close\r\n"
    if corps:
        tete += "Content-Type: application/json\r\n"
    ecrivain.write(tete.encode() + b"\r\n" + corps)
    await ecrivain.drain()


async def _traiter(
    lecteur: asyncio.StreamReader,
    ecrivain: asyncio.StreamWriter,
    *,
    sur_evenement: Callable[[Evenement], None],
    etat: Callable[[], dict],
    projets_claude: Path,
) -> None:
    try:
        tete = await asyncio.wait_for(lecteur.readuntil(b"\r\n\r\n"), DELAI_S)
        lignes = tete.decode("latin-1").split("\r\n")
        methode, chemin, _ = lignes[0].split(" ", 2)
        entetes = {}
        for ligne in lignes[1:]:
            if ":" in ligne:
                cle, valeur = ligne.split(":", 1)
                entetes[cle.strip().lower()] = valeur.strip()
        if methode == "GET" and chemin == "/etat":
            await _repondre(ecrivain, 200, json.dumps(etat()).encode())
            return
        if methode != "POST" or chemin != "/event":
            await _repondre(ecrivain, 404)
            return
        if "content-length" not in entetes:
            await _repondre(ecrivain, 411)
            return
        longueur = int(entetes["content-length"])
        if not 0 <= longueur <= TAILLE_MAX:
            await _repondre(ecrivain, 413)
            return
        if entetes.get("expect", "").lower() == "100-continue":
            ecrivain.write(b"HTTP/1.1 100 Continue\r\n\r\n")
            await ecrivain.drain()
        corps = await asyncio.wait_for(lecteur.readexactly(longueur), DELAI_S)
        ev = filtrer(corps, entetes.get("x-claude-pid"), time.time(), projets_claude)
        del corps
        if ev is not None:
            try:
                sur_evenement(ev)
            except Exception:
                log.exception("événement %s en erreur", ev.nom)
                await _repondre(ecrivain, 500)
                return
        await _repondre(ecrivain, 204)
    except (asyncio.IncompleteReadError, asyncio.LimitOverrunError, TimeoutError, ValueError, ConnectionError) as e:
        log.debug("requête rejetée : %s", type(e).__name__)
    finally:
        ecrivain.close()
        with suppress(Exception):
            await ecrivain.wait_closed()


async def demarrer_serveur(
    chemin_socket: Path,
    sur_evenement: Callable[[Evenement], None],
    etat: Callable[[], dict],
    projets_claude: Path,
) -> asyncio.Server:
    dossier = chemin_socket.parent
    dossier.mkdir(parents=True, exist_ok=True)
    os.chmod(dossier, 0o700)
    if chemin_socket.exists():
        try:
            _, ecrivain = await asyncio.open_unix_connection(str(chemin_socket))
        except OSError:
            chemin_socket.unlink()  # socket orphelin d'un daemon mort
        else:
            ecrivain.close()
            raise DejaLance(str(chemin_socket))
    gestionnaire = functools.partial(
        _traiter, sur_evenement=sur_evenement, etat=etat, projets_claude=projets_claude
    )
    ancien = os.umask(0o177)
    try:
        serveur = await asyncio.start_unix_server(gestionnaire, path=str(chemin_socket))
    finally:
        os.umask(ancien)
    os.chmod(chemin_socket, 0o600)
    return serveur


def interroger_etat(chemin_socket: Path, delai_s: float = 1.0) -> dict | None:
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:
            s.settimeout(delai_s)
            s.connect(str(chemin_socket))
            s.sendall(b"GET /etat HTTP/1.1\r\nHost: localhost\r\n\r\n")
            reponse = b""
            while morceau := s.recv(4096):
                reponse += morceau
    except OSError:
        return None
    tete, _, corps = reponse.partition(b"\r\n\r\n")
    if not tete.startswith(b"HTTP/1.1 200"):
        return None
    try:
        return json.loads(corps)
    except ValueError:
        return None
```

- [ ] **Step 5: Vérifier que les tests passent**

Run: `uv run pytest tests/test_serveur.py -q`
Expected : PASS, 28 tests.

- [ ] **Step 6: Commit**

```bash
git add src/discord_rich_claude/serveur.py tests/conftest.py tests/test_serveur.py
git commit -m "feat: receive hook events on a private unix socket"
```

---

### Task 8: Assemblage du daemon, commande `run`, carte en direct

**Files:**
- Create: `src/discord_rich_claude/daemon.py`
- Create: `src/discord_rich_claude/cli.py`
- Create (non commité) : `.claude/settings.local.json`
- Test: `tests/test_daemon.py`
- Modify: `docs/superpowers/specs/2026-09-18-discord-rich-claude-design.md` (section 12, H5)

**Interfaces:**
- Consumes : `chemins.*` et `sessions.Registre`, `sessions.pid_vivant`, `sessions.Evenement` (Task 2) ; `config.ConfigSurveillee` (Task 3) ; `carte.Carte`, `carte.Stats`, `carte.construire_carte`, `carte.est_vue_b` (Task 4) ; `tokens.SuiviTokens`, `tokens.minuit_local` (Task 5) ; `presence.Presence`, `presence.ClientPypresence` (Task 6) ; `serveur.demarrer_serveur`, `serveur.DejaLance` (Task 7).
- Produces :
  - `daemon.Daemon(config: ConfigSurveillee, presence: Presence, suivi: SuiviTokens, projets_claude: Path, pid_vivant: Callable[[int], bool] = pid_vivant, horloge: Callable[[], float] = time.time)` : attribut `registre` ; méthodes `recevoir(ev) -> None`, `etat() -> dict`, `carte() -> Carte | None`, `async controler() -> None`, `async executer(chemin_socket: Path, arret: asyncio.Event) -> None`.
  - `daemon.configurer_logs(dossier: Path) -> None`.
  - `cli.main(argv: list[str] | None = None) -> int`, sous-commande `run`.

- [ ] **Step 1: Écrire le test bout en bout qui échoue, `tests/test_daemon.py`**

```python
import asyncio
import json
import os
import time
from datetime import datetime

from discord_rich_claude.chemins import COMMANDE_HOOK
from discord_rich_claude.config import ConfigSurveillee
from discord_rich_claude.daemon import Daemon
from discord_rich_claude.presence import Presence
from discord_rich_claude.tokens import SuiviTokens


class FauxClient:
    def __init__(self, journal):
        self.journal = journal

    async def connecter(self):
        self.journal.append("connecter")

    async def publier(self, carte):
        self.journal.append(carte)

    async def effacer(self):
        self.journal.append("effacer")

    async def fermer(self):
        self.journal.append("fermer")


async def attendre(condition, delai=5.0):
    fin = time.monotonic() + delai
    while not condition():
        if time.monotonic() > fin:
            raise AssertionError("condition jamais atteinte")
        await asyncio.sleep(0.05)


async def envoyer_hook(home, payload):
    """Lance la commande de hook exacte, comme Claude Code le ferait."""
    proc = await asyncio.create_subprocess_shell(
        COMMANDE_HOOK,
        stdin=asyncio.subprocess.PIPE,
        env={"HOME": str(home), "PATH": os.environ["PATH"]},
    )
    await proc.communicate(json.dumps(payload).encode())
    assert proc.returncode == 0


async def test_bout_en_bout(dossier_court, tmp_path):
    home = dossier_court
    socket_ = home / ".local" / "state" / "discord-rich-claude" / "daemon.sock"
    projets_claude = tmp_path / "projects"
    transcript = projets_claude / "p" / "s1.jsonl"
    transcript.parent.mkdir(parents=True)
    transcript.write_text(
        json.dumps(
            {
                "type": "assistant",
                "requestId": "r1",
                "timestamp": datetime.now().astimezone().isoformat(),
                "message": {
                    "id": "m1",
                    "model": "claude-opus-5",
                    "usage": {
                        "input_tokens": 10,
                        "output_tokens": 5,
                        "cache_creation_input_tokens": 100,
                        "cache_read_input_tokens": 9999,
                    },
                },
            }
        )
        + "\n",
        encoding="utf-8",
    )
    projet = tmp_path / "MonProjet"
    projet.mkdir()
    config = tmp_path / "config.toml"
    config.write_text(f'client_id = "42"\n[projets]\nautorises = ["{projet}"]\n', encoding="utf-8")

    journal = []
    presence = Presence(lambda cid: FauxClient(journal), intervalle_min_s=0, reessai_s=0)
    daemon = Daemon(ConfigSurveillee(config), presence, SuiviTokens(projets_claude), projets_claude)
    arret = asyncio.Event()
    tache = asyncio.create_task(daemon.executer(socket_, arret))
    await attendre(socket_.exists)

    await envoyer_hook(
        home,
        {
            "hook_event_name": "SessionStart",
            "session_id": "s1",
            "cwd": str(projet),
            "transcript_path": str(transcript),
        },
    )
    await envoyer_hook(
        home,
        {
            "hook_event_name": "PreToolUse",
            "session_id": "s1",
            "tool_name": "Edit",
            "tool_input": {"content": "SECRET-CONTENU"},
        },
    )
    await daemon.controler()

    session = daemon.registre.sessions["s1"]
    assert session.pid == os.getpid()  # $PPID du shell de hook = ce processus
    assert "SECRET" not in repr(session)
    assert daemon.etat()["sessions"] == 1

    def carte_attendue():
        cartes = [c for c in journal if not isinstance(c, str)]
        return any(
            c.details == "Écrit du code · MonProjet"
            and c.state in ("Opus 5 · 115 tokens", "1 session · 115 aujourd'hui")
            for c in cartes
        )

    await attendre(carte_attendue)

    await envoyer_hook(home, {"hook_event_name": "SessionEnd", "session_id": "s1"})
    await attendre(lambda: "effacer" in journal)

    arret.set()
    await asyncio.wait_for(tache, 5)
    assert not socket_.exists()
```

- [ ] **Step 2: Vérifier que le test échoue**

Run: `uv run pytest tests/test_daemon.py -q`
Expected : FAIL, `ModuleNotFoundError: No module named 'discord_rich_claude.daemon'`.

- [ ] **Step 3: Écrire `src/discord_rich_claude/daemon.py`**

```python
"""Assemblage du daemon : serveur, état, tokens, carte, Discord. Voir la spec, sections 3 et 4.4."""
import asyncio
import logging
import os
import time
from collections.abc import Callable
from contextlib import suppress
from logging.handlers import RotatingFileHandler
from pathlib import Path

from .carte import Carte, Stats, construire_carte, est_vue_b
from .config import ConfigSurveillee
from .presence import Presence
from .serveur import demarrer_serveur
from .sessions import Evenement, Registre, pid_vivant
from .tokens import SuiviTokens, minuit_local

log = logging.getLogger(__name__)

PERIODE_CONTROLE_S = 30
PERIODE_CARTE_S = 1


def configurer_logs(dossier: Path) -> None:
    dossier.mkdir(parents=True, exist_ok=True)
    gestionnaire = RotatingFileHandler(
        dossier / "daemon.log", maxBytes=1_000_000, backupCount=3, encoding="utf-8"
    )
    gestionnaire.setFormatter(logging.Formatter("%(asctime)s %(levelname)s %(name)s %(message)s"))
    racine = logging.getLogger()
    racine.addHandler(gestionnaire)
    racine.setLevel(os.environ.get("DRC_LOG", "INFO").upper())


class Daemon:
    def __init__(
        self,
        config: ConfigSurveillee,
        presence: Presence,
        suivi: SuiviTokens,
        projets_claude: Path,
        pid_vivant: Callable[[int], bool] = pid_vivant,
        horloge: Callable[[], float] = time.time,
    ) -> None:
        self.config = config
        self.presence = presence
        self.suivi = suivi
        self.projets_claude = projets_claude
        self.pid_vivant = pid_vivant
        self.horloge = horloge
        self.registre = Registre()
        self._par_session: dict[str, tuple[str | None, int]] = {}
        self._tokens_jour = 0
        self._verrou = asyncio.Lock()

    def recevoir(self, ev: Evenement) -> None:
        niveau = logging.INFO if ev.nom in ("SessionStart", "SessionEnd") else logging.DEBUG
        log.log(niveau, "%s session=%s", ev.nom, ev.session_id[:8])
        self.registre.appliquer(ev)

    def etat(self) -> dict:
        return {"discord": self.presence.connecte, "sessions": len(self.registre.sessions)}

    def carte(self) -> Carte | None:
        maintenant = self.horloge()
        cfg = self.config.valeur
        session = self.registre.session_affichee()
        modele, tokens = self._par_session.get(session.session_id, (None, 0)) if session else (None, 0)
        stats = Stats(modele=modele, tokens_session=tokens, tokens_jour=self._tokens_jour)
        return construire_carte(
            self.registre, cfg, stats, maintenant, est_vue_b(maintenant, cfg.rotation_s)
        )

    async def controler(self) -> None:
        # Le verrou garantit un seul appelant à la fois pour SuiviTokens, qui n'est pas thread-safe.
        async with self._verrou:
            maintenant = self.horloge()
            self.config.recharger_si_change()
            inactif_s = self.config.valeur.inactif_apres_min * 60
            self.registre.purger(maintenant, inactif_s, self.pid_vivant)
            transcripts = {
                sid: Path(s.transcript_path)
                for sid, s in self.registre.sessions.items()
                if s.transcript_path
            }
            minuit = minuit_local(maintenant)
            # Lecture disque dans un thread : les hooks continuent d'être servis.
            self._par_session, self._tokens_jour = await asyncio.to_thread(
                self._compter, transcripts, minuit
            )

    def _compter(
        self, transcripts: dict[str, Path], minuit: float
    ) -> tuple[dict[str, tuple[str | None, int]], int]:
        self.suivi.rafraichir(transcripts.values(), minuit)
        par_session = {
            sid: (self.suivi.modele(t), self.suivi.tokens_session(t)) for sid, t in transcripts.items()
        }
        return par_session, self.suivi.tokens_jour(minuit)

    async def executer(self, chemin_socket: Path, arret: asyncio.Event) -> None:
        serveur = await demarrer_serveur(chemin_socket, self.recevoir, self.etat, self.projets_claude)
        log.info("daemon à l'écoute sur %s", chemin_socket)
        taches = [
            asyncio.create_task(self._boucle(self.controler, PERIODE_CONTROLE_S, "contrôle")),
            asyncio.create_task(self._boucle(self._publier, PERIODE_CARTE_S, "carte")),
        ]
        try:
            await arret.wait()
        finally:
            for tache in taches:
                tache.cancel()
            await asyncio.gather(*taches, return_exceptions=True)
            serveur.close()
            await serveur.wait_closed()
            await self.presence.fermer()
            with suppress(FileNotFoundError):
                chemin_socket.unlink()
            log.info("daemon arrêté")

    async def _publier(self) -> None:
        await self.presence.soumettre(self.carte(), self.config.valeur.client_id)

    @staticmethod
    async def _boucle(action: Callable, periode_s: float, nom: str) -> None:
        while True:
            try:
                await action()
            except Exception:
                log.exception("%s en erreur", nom)
            await asyncio.sleep(periode_s)
```

- [ ] **Step 4: Écrire `src/discord_rich_claude/cli.py` (sous-commande `run` seulement)**

```python
"""Ligne de commande. Voir la spec, section 8."""
import argparse
import asyncio
import logging
import signal

from . import chemins
from .config import ConfigSurveillee
from .daemon import Daemon, configurer_logs
from .presence import ClientPypresence, Presence
from .serveur import DejaLance
from .tokens import SuiviTokens

log = logging.getLogger(__name__)


def lancer() -> int:
    configurer_logs(chemins.DOSSIER_LOGS)

    async def principal() -> None:
        arret = asyncio.Event()
        boucle = asyncio.get_running_loop()
        for sig in (signal.SIGTERM, signal.SIGINT):
            boucle.add_signal_handler(sig, arret.set)
        daemon = Daemon(
            ConfigSurveillee(chemins.CONFIG),
            Presence(ClientPypresence),
            SuiviTokens(chemins.PROJETS_CLAUDE),
            chemins.PROJETS_CLAUDE,
        )
        await daemon.executer(chemins.SOCKET, arret)

    try:
        asyncio.run(principal())
    except DejaLance:
        log.error("un daemon tourne déjà sur %s", chemins.SOCKET)
        return 1
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="discord-rich-claude", description="Discord Rich Presence pour Claude Code"
    )
    sous = parser.add_subparsers(dest="commande", required=True)
    sous.add_parser("run", help="lance le daemon (utilisé par le LaunchAgent)")
    args = parser.parse_args(argv)
    return {"run": lancer}[args.commande]()
```

- [ ] **Step 5: Vérifier que le test passe, puis toute la suite**

Run: `uv run pytest tests/test_daemon.py -q && uv run pytest -q`
Expected : PASS, 1 test, puis 120 tests (83 + 8 + 28 + 1). Rapporter le nombre réellement exécuté.

- [ ] **Step 6: Commit**

```bash
git add src/discord_rich_claude/daemon.py src/discord_rich_claude/cli.py tests/test_daemon.py
git commit -m "feat: assemble daemon and run command"
```

- [ ] **Step 7: CHECKPOINT MANUEL — Victor crée l'application Discord**

Arrêter et donner à Victor ces instructions :

1. Ouvrir https://discord.com/developers/applications, « New Application », nom : `Claude Code`.
2. « Rich Presence » > « Art Assets » : déposer le logo, clé `logo`. Les assets peuvent mettre quelques minutes à apparaître.
3. « General Information » : copier l'Application ID.
4. Discord desktop : Paramètres > Confidentialité de l'activité > « Partager mon activité » activé.

Puis créer la config avec l'ID donné par Victor (remplacer `<ID>`) :

```bash
mkdir -p ~/.config/discord-rich-claude
cat > ~/.config/discord-rich-claude/config.toml <<'EOF'
client_id = "<ID>"
inactif_apres_min = 10
rotation_s = 20

[projets]
autorises = [
  "/Volumes/MAC-Extended/Workspace-MAC/DiscordRichClaude",
]
EOF
```

- [ ] **Step 8: Brancher les hooks sur ce projet seulement**

Vérifier que `.claude/settings.local.json` n'existe pas (la sonde de la Task 1 a été retirée). S'il existe : arrêter et demander à Victor. Sinon le générer depuis le code, pour qu'il porte la commande exacte :

```bash
uv run python - <<'EOF'
import json
from pathlib import Path
from discord_rich_claude.chemins import COMMANDE_HOOK
evenements = ["SessionStart", "UserPromptSubmit", "PreToolUse", "PermissionRequest",
              "Stop", "SubagentStart", "SubagentStop", "SessionEnd"]
hooks = {}
for ev in evenements:
    groupe = {"hooks": [{"type": "command", "command": COMMANDE_HOOK, "timeout": 2}]}
    if ev == "PreToolUse":
        groupe = {"matcher": "*", **groupe}
    hooks[ev] = [groupe]
Path(".claude").mkdir(exist_ok=True)
Path(".claude/settings.local.json").write_text(json.dumps({"hooks": hooks}, indent=2) + "\n")
EOF
```

- [ ] **Step 9: Lancer le daemon et mesurer la latence d'un hook (H5)**

Dans un terminal dédié : `DRC_LOG=DEBUG uv run discord-rich-claude run`. Dans un autre : `tail -f ~/Library/Logs/discord-rich-claude/daemon.log`.

Mesure de latence, depuis un troisième terminal :
```bash
CMD='curl -s --max-time 1 --unix-socket "$HOME/.local/state/discord-rich-claude/daemon.sock" -H "X-Claude-Pid: $PPID" --data-binary @- http://localhost/event >/dev/null 2>&1 || true'
time (for i in $(seq 50); do echo '{"hook_event_name":"UserPromptSubmit","session_id":"mesure-latence"}' | sh -c "$CMD"; done)
echo '{"hook_event_name":"SessionEnd","session_id":"mesure-latence"}' | sh -c "$CMD"
```
Expected : temps réel / 50 de l'ordre de 10 ms. Noter la valeur mesurée dans la section 12 de la spec (H5). Au-delà de 100 ms par hook, arrêter et en parler à Victor : la spec prévoit des hooks synchrones sur l'hypothèse d'un coût négligeable.

- [ ] **Step 10: CHECKPOINT MANUEL — preuve du lot L3**

Victor ouvre une nouvelle session `claude` dans ce dossier et vérifie, depuis un autre compte Discord ou son téléphone :

1. La carte « Claude Code » apparaît avec « Attend ta réponse · DiscordRichClaude ».
2. Un prompt qui lit et édite un fichier : l'activité change (avec jusqu'à 15 s de retard, limite Discord).
3. Au bout de 20 s, la ligne 2 alterne entre « Opus 5 · … tokens » et « 1 session · … aujourd'hui ».
4. `/exit` : la carte disparaît en moins de 15 s.
5. Dans le log : aucune trace de prompt ni de chemin de fichier édité (`grep -i -E "prompt|tool_input|content" ~/Library/Logs/discord-rich-claude/daemon.log` ne renvoie que des noms d'événements).

- [ ] **Step 11: Commit de la mesure, merge du lot**

Arrêter le daemon (Ctrl-C) et laisser `.claude/settings.local.json` en place jusqu'à la Task 11.

```bash
git add docs/superpowers/specs/2026-09-18-discord-rich-claude-design.md
git commit -m "docs: record hook latency measurement"
git checkout develop && git merge --no-ff feature/l3-daemon -m "merge: L3 live daemon"
```

---

## Lot L4 — installation

### Task 9: Ajout et retrait des hooks dans settings.json

**Files:**
- Create: `src/discord_rich_claude/installation.py`
- Test: `tests/test_installation.py`

**Interfaces:**
- Consumes : `chemins.COMMANDE_HOOK` (Task 2).
- Produces :
  - `installation.EVENEMENTS_HOOKS: tuple[str, ...]` (les 8 événements de la spec 4.1), `installation.MARQUEUR = "discord-rich-claude"`.
  - `installation.ajouter_hooks(settings: dict, commande: str = COMMANDE_HOOK) -> dict` et `installation.retirer_hooks(settings: dict) -> dict` : pures, ne modifient pas l'argument.
  - `installation.modifier_settings(chemin: Path, transformer: Callable[[dict], dict], confirmer: Callable[[str], bool], horloge: Callable[[], datetime] = datetime.now) -> str`, qui rend `"modifié"`, `"inchangé"` ou `"refusé"` ; lève `ValueError` si le fichier n'est pas un objet JSON valide, sans rien écrire.

- [ ] **Step 1: Créer la branche du lot**

```bash
git checkout develop && git checkout -b feature/l4-installation
```

- [ ] **Step 2: Écrire les tests qui échouent, `tests/test_installation.py`**

```python
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
```

- [ ] **Step 3: Vérifier que les tests échouent**

Run: `uv run pytest tests/test_installation.py -q`
Expected : FAIL, `ModuleNotFoundError: No module named 'discord_rich_claude.installation'`.

- [ ] **Step 4: Écrire `src/discord_rich_claude/installation.py`**

```python
"""Installation : hooks dans ~/.claude/settings.json. Voir la spec, section 8.

Nos hooks se reconnaissent au marqueur dans leur commande. Les hooks d'autres
outils ne sont jamais touchés, même dans un groupe mixte.
"""
import copy
import difflib
import json
import os
from collections.abc import Callable
from datetime import datetime
from pathlib import Path

from .chemins import COMMANDE_HOOK

MARQUEUR = "discord-rich-claude"
EVENEMENTS_HOOKS = (
    "SessionStart", "UserPromptSubmit", "PreToolUse", "PermissionRequest",
    "Stop", "SubagentStart", "SubagentStop", "SessionEnd",
)


def _est_a_nous(hook: object) -> bool:
    return isinstance(hook, dict) and MARQUEUR in str(hook.get("command", ""))


def _sans_les_notres(groupes: list) -> tuple[list, bool]:
    resultat, retire = [], False
    for groupe in groupes:
        hooks = groupe.get("hooks", []) if isinstance(groupe, dict) else []
        gardes = [h for h in hooks if not _est_a_nous(h)]
        if len(gardes) != len(hooks):
            retire = True
            if not gardes:
                continue
            groupe = {**groupe, "hooks": gardes}
        resultat.append(groupe)
    return resultat, retire


def ajouter_hooks(settings: dict, commande: str = COMMANDE_HOOK) -> dict:
    nouveau = copy.deepcopy(settings)
    hooks = nouveau.setdefault("hooks", {})
    for ev in EVENEMENTS_HOOKS:
        groupes, _ = _sans_les_notres(hooks.get(ev, []))
        groupe = {"hooks": [{"type": "command", "command": commande, "timeout": 2}]}
        if ev == "PreToolUse":
            groupe = {"matcher": "*", **groupe}
        hooks[ev] = [*groupes, groupe]
    return nouveau


def retirer_hooks(settings: dict) -> dict:
    nouveau = copy.deepcopy(settings)
    hooks = nouveau.get("hooks")
    if not isinstance(hooks, dict):
        return nouveau
    for ev in list(hooks):
        groupes, retire = _sans_les_notres(hooks[ev])
        if retire and not groupes:
            del hooks[ev]
        else:
            hooks[ev] = groupes
    if not hooks:
        del nouveau["hooks"]
    return nouveau


def modifier_settings(
    chemin: Path,
    transformer: Callable[[dict], dict],
    confirmer: Callable[[str], bool],
    horloge: Callable[[], datetime] = datetime.now,
) -> str:
    try:
        ancien = json.loads(chemin.read_text(encoding="utf-8"))
    except FileNotFoundError:
        ancien = {}
    if not isinstance(ancien, dict):
        raise ValueError(f"{chemin} ne contient pas un objet JSON")
    nouveau = transformer(ancien)
    if nouveau == ancien:
        return "inchangé"
    avant = json.dumps(ancien, indent=2, ensure_ascii=False).splitlines()
    apres = json.dumps(nouveau, indent=2, ensure_ascii=False).splitlines()
    diff = "\n".join(difflib.unified_diff(avant, apres, "avant", "après", lineterm=""))
    if not confirmer(diff):
        return "refusé"
    if chemin.exists():
        sauvegarde = chemin.with_name(f"{chemin.name}.bak-{horloge():%Y%m%d-%H%M%S}")
        sauvegarde.write_bytes(chemin.read_bytes())
    chemin.parent.mkdir(parents=True, exist_ok=True)
    temporaire = chemin.with_name(f"{chemin.name}.tmp")
    temporaire.write_text(json.dumps(nouveau, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    os.replace(temporaire, chemin)
    return "modifié"
```

- [ ] **Step 5: Vérifier que les tests passent**

Run: `uv run pytest tests/test_installation.py -q`
Expected : PASS, 12 tests.

- [ ] **Step 6: Commit**

```bash
git add src/discord_rich_claude/installation.py tests/test_installation.py
git commit -m "feat: add and remove our hooks in claude settings"
```

---

### Task 10: LaunchAgent, config par défaut et commandes install, uninstall, status

**Files:**
- Modify: `src/discord_rich_claude/installation.py` (ajout en fin de fichier)
- Modify: `src/discord_rich_claude/config.py` (ajout en fin de fichier)
- Modify: `src/discord_rich_claude/cli.py` (réécriture complète)
- Test: `tests/test_installation.py` (ajouts), `tests/test_config.py` (ajouts), `tests/test_cli.py`

**Interfaces:**
- Consumes : `installation.ajouter_hooks`, `installation.retirer_hooks`, `installation.modifier_settings` (Task 9) ; `serveur.interroger_etat` (Task 7) ; `cli.lancer` (Task 8) ; `chemins.*` (Task 2).
- Produces :
  - `installation.generer_plist(executable: str, dossier_logs: Path) -> bytes`.
  - `installation.installer_launch_agent(chemin_plist: Path, contenu: bytes, executer: Callable = subprocess.run) -> None`, `installation.desinstaller_launch_agent(chemin_plist: Path, executer: Callable = subprocess.run) -> None`.
  - `config.CONFIG_PAR_DEFAUT: str`, `config.creer_config_par_defaut(chemin: Path) -> bool` (False si le fichier existe déjà, sans le toucher).
  - `cli.main` avec `run`, `install`, `uninstall`, `status`.

- [ ] **Step 1: Ajouter les tests qui échouent**

À la fin de `tests/test_installation.py` :

```python
import os
import plistlib

from discord_rich_claude.chemins import LABEL
from discord_rich_claude.installation import (
    desinstaller_launch_agent,
    generer_plist,
    installer_launch_agent,
)


def test_generer_plist(tmp_path):
    plist = plistlib.loads(generer_plist("/u/bin/discord-rich-claude", tmp_path))
    assert plist["Label"] == LABEL
    assert plist["ProgramArguments"] == ["/u/bin/discord-rich-claude", "run"]
    assert plist["RunAtLoad"] is True
    assert plist["KeepAlive"] is True
    assert plist["StandardErrorPath"] == str(tmp_path / "launchd.err.log")


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
```

À la fin de `tests/test_config.py` :

```python
from discord_rich_claude.config import creer_config_par_defaut


def test_config_par_defaut_creee_et_refuse_tout(tmp_path):
    f = tmp_path / "sous" / "config.toml"
    assert creer_config_par_defaut(f) is True
    assert charger(f) == Config()


def test_config_par_defaut_n_ecrase_pas(tmp_path):
    f = tmp_path / "config.toml"
    f.write_text('client_id = "1"\n', encoding="utf-8")
    assert creer_config_par_defaut(f) is False
    assert f.read_text(encoding="utf-8") == 'client_id = "1"\n'
```

Créer `tests/test_cli.py` :

```python
import pytest

from discord_rich_claude import cli


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
```

- [ ] **Step 2: Vérifier que les tests échouent**

Run: `uv run pytest tests/test_installation.py tests/test_config.py tests/test_cli.py -q`
Expected : FAIL, `ImportError` sur `generer_plist`, `creer_config_par_defaut` et `cli.interroger_etat`.

- [ ] **Step 3: Ajouter à la fin de `src/discord_rich_claude/config.py`**

```python
CONFIG_PAR_DEFAUT = """\
# Configuration de discord-rich-claude.
# ID de l'application Discord : Developer Portal > General Information > Application ID.
client_id = ""
inactif_apres_min = 10
rotation_s = 20

[projets]
# Refusé par défaut : seuls ces dossiers (chemins complets) affichent leur nom.
autorises = []
"""


def creer_config_par_defaut(chemin: Path) -> bool:
    if chemin.exists():
        return False
    chemin.parent.mkdir(parents=True, exist_ok=True)
    chemin.write_text(CONFIG_PAR_DEFAUT, encoding="utf-8")
    return True
```

- [ ] **Step 4: Ajouter à la fin de `src/discord_rich_claude/installation.py`**

Ajouter `import plistlib` et `import subprocess` aux imports, et `from .chemins import COMMANDE_HOOK, LABEL` à la place de l'import existant. Puis en fin de fichier :

```python
def generer_plist(executable: str, dossier_logs: Path) -> bytes:
    return plistlib.dumps(
        {
            "Label": LABEL,
            "ProgramArguments": [executable, "run"],
            "RunAtLoad": True,
            "KeepAlive": True,
            "ProcessType": "Background",
            "StandardOutPath": str(dossier_logs / "launchd.out.log"),
            "StandardErrorPath": str(dossier_logs / "launchd.err.log"),
        }
    )


def installer_launch_agent(
    chemin_plist: Path, contenu: bytes, executer: Callable = subprocess.run
) -> None:
    domaine = f"gui/{os.getuid()}"
    # Décharge une version précédente ; échoue sans conséquence si rien n'est chargé.
    executer(["launchctl", "bootout", f"{domaine}/{LABEL}"], check=False, capture_output=True)
    chemin_plist.parent.mkdir(parents=True, exist_ok=True)
    chemin_plist.write_bytes(contenu)
    executer(["launchctl", "bootstrap", domaine, str(chemin_plist)], check=True, capture_output=True)


def desinstaller_launch_agent(chemin_plist: Path, executer: Callable = subprocess.run) -> None:
    executer(
        ["launchctl", "bootout", f"gui/{os.getuid()}/{LABEL}"], check=False, capture_output=True
    )
    chemin_plist.unlink(missing_ok=True)
```

- [ ] **Step 5: Réécrire `src/discord_rich_claude/cli.py` en entier**

```python
"""Ligne de commande. Voir la spec, section 8."""
import argparse
import asyncio
import logging
import shutil
import signal
import subprocess
import sys

from . import chemins
from .config import ConfigSurveillee, creer_config_par_defaut
from .daemon import Daemon, configurer_logs
from .installation import (
    ajouter_hooks,
    desinstaller_launch_agent,
    generer_plist,
    installer_launch_agent,
    modifier_settings,
    retirer_hooks,
)
from .presence import ClientPypresence, Presence
from .serveur import DejaLance, interroger_etat
from .tokens import SuiviTokens

log = logging.getLogger(__name__)


def lancer() -> int:
    configurer_logs(chemins.DOSSIER_LOGS)

    async def principal() -> None:
        arret = asyncio.Event()
        boucle = asyncio.get_running_loop()
        for sig in (signal.SIGTERM, signal.SIGINT):
            boucle.add_signal_handler(sig, arret.set)
        daemon = Daemon(
            ConfigSurveillee(chemins.CONFIG),
            Presence(ClientPypresence),
            SuiviTokens(chemins.PROJETS_CLAUDE),
            chemins.PROJETS_CLAUDE,
        )
        await daemon.executer(chemins.SOCKET, arret)

    try:
        asyncio.run(principal())
    except DejaLance:
        log.error("un daemon tourne déjà sur %s", chemins.SOCKET)
        return 1
    return 0


def _confirmer(diff: str) -> bool:
    print(diff)
    return input(f"Appliquer ces changements à {chemins.SETTINGS_CLAUDE} ? [o/N] ").strip().lower() == "o"


def installer() -> int:
    print("1/3 Hooks Claude Code")
    try:
        resultat = modifier_settings(chemins.SETTINGS_CLAUDE, ajouter_hooks, _confirmer)
    except ValueError as e:
        print(f"  settings.json illisible, rien n'a été modifié : {e}")
        return 1
    if resultat == "refusé":
        print("  installation annulée, rien n'a été modifié")
        return 1
    print(f"  {resultat}")
    print("2/3 Configuration")
    if creer_config_par_defaut(chemins.CONFIG):
        print(f"  créée : {chemins.CONFIG}. Renseigne client_id et projets.autorises.")
    else:
        print(f"  déjà présente : {chemins.CONFIG}")
    print("3/3 LaunchAgent")
    chemins.DOSSIER_LOGS.mkdir(parents=True, exist_ok=True)
    executable = shutil.which("discord-rich-claude") or sys.argv[0]
    try:
        installer_launch_agent(chemins.LAUNCH_AGENT, generer_plist(executable, chemins.DOSSIER_LOGS))
    except subprocess.CalledProcessError as e:
        print(f"  launchctl a échoué ({e.returncode}) : relance `discord-rich-claude install`")
        return 1
    print("  daemon démarré")
    return 0


def desinstaller() -> int:
    try:
        resultat = modifier_settings(chemins.SETTINGS_CLAUDE, retirer_hooks, _confirmer)
    except ValueError as e:
        print(f"settings.json illisible, rien n'a été modifié : {e}")
        return 1
    if resultat == "refusé":
        print("désinstallation annulée")
        return 1
    desinstaller_launch_agent(chemins.LAUNCH_AGENT)
    print(f"Désinstallé. Config conservée : {chemins.CONFIG}")
    return 0


def statut() -> int:
    etat = interroger_etat(chemins.SOCKET)
    if etat is None:
        print("daemon : injoignable")
        return 1
    print("daemon : actif")
    print(f"discord : {'connecté' if etat.get('discord') else 'non connecté'}")
    print(f"sessions : {etat.get('sessions', 0)}")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="discord-rich-claude", description="Discord Rich Presence pour Claude Code"
    )
    sous = parser.add_subparsers(dest="commande", required=True)
    sous.add_parser("run", help="lance le daemon (utilisé par le LaunchAgent)")
    sous.add_parser("install", help="ajoute les hooks, crée la config et le LaunchAgent")
    sous.add_parser("uninstall", help="retire nos hooks et le LaunchAgent, garde la config")
    sous.add_parser("status", help="état du daemon")
    args = parser.parse_args(argv)
    commandes = {"run": lancer, "install": installer, "uninstall": desinstaller, "status": statut}
    return commandes[args.commande]()
```

- [ ] **Step 6: Vérifier que les tests passent, puis toute la suite**

Run: `uv run pytest tests/test_installation.py tests/test_config.py tests/test_cli.py -q && uv run pytest -q`
Expected : PASS, 3 + 2 + 3 nouveaux tests ; suite complète à 140 tests (120 + 12 + 8). Rapporter le nombre réellement exécuté.

- [ ] **Step 7: Commit**

```bash
git add src/discord_rich_claude/installation.py src/discord_rich_claude/config.py src/discord_rich_claude/cli.py tests/test_installation.py tests/test_config.py tests/test_cli.py
git commit -m "feat: add install, uninstall and status commands"
```

---

### Task 11: Documentation, installation réelle et protocole manuel

**Files:**
- Create: `README.md`
- Create: `CLAUDE.md`
- Delete (non commité) : `.claude/settings.local.json`

**Interfaces:**
- Consumes : toutes les tâches précédentes.
- Produces : l'outil installé sur la machine de Victor ; la preuve du lot L4.

- [ ] **Step 1: Écrire `README.md`**

````markdown
# discord-rich-claude

Affiche sur ton profil Discord une carte « Joue à Claude Code » quand une session Claude Code tourne : activité en cours, modèle, tokens, durée. Le nom du projet n'apparaît que pour les dossiers que tu autorises.

Tout reste local : les hooks de Claude Code parlent au daemon par un socket Unix, le daemon parle à Discord par sa connexion locale. Aucune donnée ne part sur le réseau.

## Installation

1. Crée l'application Discord : https://discord.com/developers/applications > New Application, nom `Claude Code`. Dans Rich Presence > Art Assets, dépose le logo avec la clé `logo`. Copie l'Application ID (General Information).
2. Installe l'outil :

   ```bash
   uv tool install /chemin/vers/DiscordRichClaude
   discord-rich-claude install
   ```

   `install` sauvegarde `~/.claude/settings.json`, te montre les hooks ajoutés et attend ton accord. Il crée ensuite la config et le LaunchAgent qui lance le daemon à l'ouverture de ta session macOS.
3. Renseigne `~/.config/discord-rich-claude/config.toml` :

   ```toml
   client_id = "ton Application ID"
   inactif_apres_min = 10
   rotation_s = 20

   [projets]
   autorises = ["/chemin/complet/vers/un/projet"]
   ```

   La config est relue toute seule quand elle change.

## Ce qui s'affiche

- Ligne 1 : l'activité (« Écrit du code », « Lance une commande », « Pilote 3 sous-agents », « Attend ta réponse », « Inactif »…), puis le nom du projet s'il est autorisé, sinon « Projet privé ».
- Ligne 2, en alternance toutes les `rotation_s` secondes : le modèle et les tokens de la session, puis le nombre de sessions et le cumul du jour.
- Le chrono part de l'ouverture de la plus ancienne session encore ouverte.

Les tokens comptés excluent les lectures du cache. Discord limite la mise à jour de la carte à une fois toutes les 15 s : l'activité peut avoir jusqu'à 15 s de retard.

## Confidentialité

- Refus par défaut : un projet absent de `projets.autorises` s'affiche « Projet privé ». La comparaison porte sur le chemin complet.
- Jamais affiché : noms de fichiers, commandes, prompts, branche git, chemins.
- Le daemon ne garde que quelques champs de chaque hook (identifiant de session, dossier, type d'événement, nom d'outil). Le texte des prompts et le contenu des outils sont jetés à la réception et jamais écrits dans le log.

## Commandes

| Commande | Effet |
|---|---|
| `discord-rich-claude status` | Daemon joignable, Discord connecté, nombre de sessions |
| `discord-rich-claude install` | Hooks, config, LaunchAgent (relançable sans risque) |
| `discord-rich-claude uninstall` | Retire nos hooks et le LaunchAgent, garde la config |
| `discord-rich-claude run` | Lance le daemon au premier plan (le LaunchAgent le fait pour toi) |

## Dépannage

- Log : `~/Library/Logs/discord-rich-claude/daemon.log`. Plus de détail : `DRC_LOG=DEBUG discord-rich-claude run` après `launchctl bootout gui/$(id -u)/local.discord-rich-claude`.
- Pas de carte : vérifie `client_id`, que Discord desktop tourne, et Paramètres Discord > Confidentialité de l'activité > « Partager mon activité ».
- Le daemon arrêté ne gêne jamais Claude Code : les hooks échouent sans bruit.

## Développement

```bash
uv sync
uv run pytest -q
```

Spec : `docs/superpowers/specs/2026-09-18-discord-rich-claude-design.md`.
````

- [ ] **Step 2: Écrire `CLAUDE.md`**

```markdown
# CLAUDE.md

Daemon Python qui affiche une Discord Rich Presence pour Claude Code. La spec fait foi : `docs/superpowers/specs/2026-09-18-discord-rich-claude-design.md`.

## Commandes

- Tests : `uv run pytest -q`
- Daemon au premier plan : `DRC_LOG=DEBUG uv run discord-rich-claude run`

## Structure

`src/discord_rich_claude/` : `sessions` (état, pur), `carte` (carte, pur), `tokens` (transcripts), `config`, `serveur` (socket Unix, filtrage), `presence` (Discord), `daemon` (assemblage), `installation` et `cli`.

## Règles

- Confidentialité : `serveur.filtrer` ne garde qu'une liste fermée de champs. N'en ajouter un qu'avec une décision écrite dans la spec. Jamais de prompt, de `tool_input`, de chemin de fichier édité dans un log ou sur Discord.
- Liste blanche des projets : refus par défaut, comparaison de chemins résolus, jamais de nom de dossier seul.
- Aucun accès réseau. Seule dépendance d'exécution : pypresence.
- `sessions` et `carte` restent sans entrée-sortie.
- Discord est mocké dans les tests (faux client), le socket et `curl` sont réels.
- Libellés en français, ceux de la spec.
- Sous macOS, un chemin de socket Unix est limité à 104 octets : les tests de socket utilisent la fixture `dossier_court`.
```

- [ ] **Step 3: Commit**

```bash
git add README.md CLAUDE.md
git commit -m "docs: add README and CLAUDE.md"
```

- [ ] **Step 4: Installation réelle**

Retirer d'abord les hooks de projet de la Task 8, sinon chaque événement arriverait deux fois :

```bash
rm .claude/settings.local.json
uv tool install .
discord-rich-claude install
```

`install` montre la différence sur `~/.claude/settings.json` : **Victor la lit et répond lui-même** « o » ou non. Ne jamais répondre à sa place. La config existe déjà (Task 8) : `install` doit afficher « déjà présente ».

Vérifier :
```bash
discord-rich-claude status
launchctl print gui/$(id -u)/local.discord-rich-claude | grep -E "state|pid"
ls ~/.claude/settings.json.bak-*
```
Expected : `daemon : actif`, le service en `state = running`, une sauvegarde datée.

- [ ] **Step 5: CHECKPOINT MANUEL — protocole de test (spec, section 10)**

Victor passe chaque parcours et regarde la carte depuis un autre compte ou son téléphone :

1. Une session dans DiscordRichClaude : « … · DiscordRichClaude ».
2. Une session dans un projet non autorisé : « … · Projet privé ».
3. Deux sessions ouvertes : la carte suit la dernière active, « 2 sessions » dans la vue B.
4. Sous-agents en arrière-plan : « Pilote N sous-agents » tant qu'ils tournent, puis « Attend ta réponse ».
5. Quitter Discord puis le relancer : la carte revient en moins de 30 s.
6. `pkill -f "discord-rich-claude run"` : le LaunchAgent relance le daemon, la carte revient à l'événement suivant.
7. Fermer un terminal sans `/exit` : la session disparaît en moins de 30 s (purge par PID).
8. Fermer puis rouvrir la session macOS : `discord-rich-claude status` répond `daemon : actif` sans action.
9. `grep -c -E "SECRET|tool_input|\"prompt\"" ~/Library/Logs/discord-rich-claude/daemon.log` : 0.

Consigner le résultat de chaque parcours (OK ou écart constaté) dans le compte rendu à Victor. Un écart est un bug : `superpowers:systematic-debugging`, puis retour à ce protocole.

- [ ] **Step 6: Suite complète, merge final**

Run: `uv run pytest -q`
Expected : PASS, 140 tests. Rapporter le nombre réellement exécuté.

```bash
git checkout develop && git merge --no-ff feature/l4-installation -m "merge: L4 installation"
```

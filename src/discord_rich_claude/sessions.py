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

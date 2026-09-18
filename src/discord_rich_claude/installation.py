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

"""Installation : hooks dans ~/.claude/settings.json. Voir la spec, section 8.

Nos hooks se reconnaissent au marqueur dans leur commande. Les hooks d'autres
outils ne sont jamais touchés, même dans un groupe mixte. Toute forme qu'on ne
sait pas modifier sans risque (valeur d'événement qui n'est pas une liste,
groupe qui n'est pas un objet ou dont les hooks ne sont pas une liste) est
laissée intacte plutôt que devinée ; quand on doit ajouter nos hooks dans une
forme pareille, on refuse avec une erreur claire plutôt que de l'abîmer.
"""
import copy
import difflib
import json
import os
import plistlib
import stat
import subprocess
import tempfile
from collections.abc import Callable
from datetime import datetime
from pathlib import Path

from .chemins import COMMANDE_HOOK, LABEL

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
        if not isinstance(groupe, dict) or not isinstance(groupe.get("hooks", []), list):
            resultat.append(groupe)
            continue
        hooks = groupe.get("hooks", [])
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
    if "hooks" in nouveau and not isinstance(nouveau["hooks"], dict):
        raise ValueError("settings['hooks'] doit être un objet")
    brut = nouveau.get("hooks", {})
    for ev in EVENEMENTS_HOOKS:
        if ev in brut and not isinstance(brut[ev], list):
            raise ValueError(f"settings['hooks']['{ev}'] doit être une liste")
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
    if "hooks" not in nouveau:
        return nouveau
    hooks = nouveau["hooks"]
    if not isinstance(hooks, dict):
        raise ValueError("settings['hooks'] doit être un objet")
    a_supprime_un_evenement = False
    for ev in list(hooks):
        valeur = hooks[ev]
        if not isinstance(valeur, list):
            continue
        groupes, retire = _sans_les_notres(valeur)
        if retire and not groupes:
            del hooks[ev]
            a_supprime_un_evenement = True
        else:
            hooks[ev] = groupes
    if a_supprime_un_evenement and not hooks:
        del nouveau["hooks"]
    return nouveau


def _lire_bytes(chemin: Path) -> bytes | None:
    try:
        return chemin.read_bytes()
    except FileNotFoundError:
        return None


def _sauvegarder(chemin: Path, contenu: bytes, mode: int, horloge: Callable[[], datetime]) -> None:
    """Sauvegarde exclusive : jamais d'écrasement d'une sauvegarde existante."""
    base = chemin.with_name(f"{chemin.name}.bak-{horloge():%Y%m%d-%H%M%S}")
    suffixe = 0
    while True:
        nom = base if suffixe == 0 else base.with_name(f"{base.name}-{suffixe}")
        try:
            with open(nom, "xb") as f:
                f.write(contenu)
        except FileExistsError:
            suffixe += 1
            continue
        os.chmod(nom, mode)
        return


def modifier_settings(
    chemin: Path,
    transformer: Callable[[dict], dict],
    confirmer: Callable[[str], bool],
    horloge: Callable[[], datetime] = datetime.now,
) -> str:
    avant_bytes = _lire_bytes(chemin)
    ancien = json.loads(avant_bytes.decode("utf-8")) if avant_bytes is not None else {}
    if not isinstance(ancien, dict):
        raise ValueError(f"{chemin} ne contient pas un objet JSON")
    nouveau = transformer(ancien)
    if nouveau == ancien:
        return "inchangé"
    avant_lignes = json.dumps(ancien, indent=2, ensure_ascii=False).splitlines()
    apres_lignes = json.dumps(nouveau, indent=2, ensure_ascii=False).splitlines()
    diff = "\n".join(difflib.unified_diff(avant_lignes, apres_lignes, "avant", "après", lineterm=""))
    if not confirmer(diff):
        return "refusé"

    # La confirmation peut prendre du temps (interaction utilisateur) ; Claude
    # Code réécrit lui-même settings.json entre-temps. On relit juste avant
    # d'écrire et on abandonne plutôt que d'écraser une modification perdue.
    apres_bytes = _lire_bytes(chemin)
    if apres_bytes != avant_bytes:
        raise ValueError(
            "settings.json a changé pendant la confirmation : relance la commande"
        )

    mode = stat.S_IMODE(chemin.stat().st_mode) if avant_bytes is not None else 0o600

    chemin.parent.mkdir(parents=True, exist_ok=True)
    if avant_bytes is not None:
        _sauvegarder(chemin, avant_bytes, mode, horloge)

    payload = (json.dumps(nouveau, indent=2, ensure_ascii=False) + "\n").encode("utf-8")
    fd, nom_temp = tempfile.mkstemp(dir=chemin.parent, prefix=".settings.json.", suffix=".tmp")
    temporaire = Path(nom_temp)
    try:
        os.fchmod(fd, mode)
        with os.fdopen(fd, "wb") as f:
            f.write(payload)
            f.flush()
            os.fsync(f.fileno())
        os.replace(temporaire, chemin)
    except BaseException:
        try:
            temporaire.unlink()
        except OSError:
            pass
        raise
    return "modifié"


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

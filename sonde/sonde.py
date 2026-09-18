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

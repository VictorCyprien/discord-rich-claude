# CLAUDE.md

Daemon Python qui affiche une Discord Rich Presence pour Claude Code. La spec fait foi : `docs/superpowers/specs/2026-09-18-discord-rich-claude-design.md`.

Le projet reste en français (code, tests, libellés, commits) : ne rien traduire, y compris dans du nouveau code.

## Commandes

- Tests : `uv run pytest -q`
- Suite sous Python 3.13 aussi (le projet cible 3.12 et 3.13) : `uv run --isolated --python 3.13 --group dev pytest -q`
- Daemon au premier plan, logs détaillés : `DRC_LOG=DEBUG uv run discord-rich-claude run`

## Structure

`src/discord_rich_claude/` : `sessions` (état, pur), `carte` (carte, pur), `tokens` (transcripts), `config`, `serveur` (socket Unix, filtrage), `presence` (Discord), `daemon` (assemblage), `chemins` (emplacements disque, commande de hook), `installation` et `cli`.

## Règles

- Confidentialité : `serveur.filtrer` ne garde qu'une liste fermée de champs (`session_id`, `cwd`, `hook_event_name`, `tool_name`, `agent_id`, `transcript_path`, l'en-tête `X-Claude-Pid`). N'en ajouter un qu'avec une décision écrite dans la spec. Jamais de prompt, de `tool_input`, de chemin de fichier édité dans un log ou sur Discord.
- Liste blanche des projets : refus par défaut, comparaison de chemins résolus, jamais de nom de dossier seul.
- Aucun accès réseau. Seule dépendance d'exécution : pypresence.
- `sessions` reste sans entrée-sortie ; `carte` aussi, sauf une résolution de chemin mise en cache par dossier (`nom_projet` : le volume qui porte un projet peut être mal monté, un `resolve()` non mis en cache bloquerait la boucle carte à 1 s).
- Discord est mocké dans les tests (faux client), le socket et `curl` sont réels.
- Libellés en français, ceux de la spec, codés en dur (jamais improvisés).
- Sous macOS, un chemin de socket Unix est limité à 104 octets : les tests de socket utilisent la fixture `dossier_court`.
- Les tests ne touchent jamais les vrais chemins utilisateur (`~/.claude`, `~/.config`, `~/Library`) : `installation` et `chemins` sont testés avec des chemins temporaires (`tmp_path`), jamais les emplacements réels. Aucun test n'appelle le vrai `launchctl` : `installer_launch_agent` / `desinstaller_launch_agent` prennent un `executer` injectable, remplacé par un faux dans les tests.
- La confirmation de `install`/`uninstall` sur `~/.claude/settings.json` (le « o » explicite) revient à Victor : ne jamais y répondre à sa place, ni la simuler, ni contourner `_confirmer`.
- La sauvegarde de `settings.json` et le fichier réécrit gardent tous les deux le mode (droits) de l'original, jamais un mode plus large même sous umask. `config.toml`, s'il est absent, est créé en 0600 avec une liste blanche vide, jamais écrasé s'il existe déjà.

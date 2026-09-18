# discord-rich-claude

Affiche sur ton profil Discord une carte « Joue à ClaudeCode » quand une session Claude Code tourne : activité en cours, modèle, tokens, durée. Le nom du projet n'apparaît que pour les dossiers que tu autorises.

Tout reste local : les hooks de Claude Code parlent au daemon par un socket Unix, le daemon parle à Discord par sa connexion locale. Aucune donnée ne part sur le réseau.

## Installation

1. Crée l'application Discord : https://discord.com/developers/applications > New Application. Le Developer Portal refuse un nom avec une espace (erreur 50277, testé avec « Claude Code ») : utilise par exemple `ClaudeCode` ou `Claude·Code`. Le titre de la carte est le nom de l'application. Dans Rich Presence > Art Assets, dépose une image carrée sous la clé d'asset `logo`. Copie l'Application ID (General Information).
2. Installe l'outil, depuis un terminal classique, sans venv de projet actif et sans passer par `uv run` : le programme lance le binaire réellement en cours d'exécution (`sys.argv[0]`), pas une recherche dans ton `PATH` ; sous `uv run`, ce binaire est celui du `.venv` du projet — ici sur un volume externe, qui peut ne pas être monté à l'ouverture de session. Installe avec `uv tool install .`, qui place l'exécutable réel sous `~/.local/bin/` :

   ```bash
   uv tool install /chemin/vers/DiscordRichClaude
   discord-rich-claude install
   ```

   `install` refuse de tourner avec `sudo`. Il te montre la différence sur `~/.claude/settings.json` et attend un « o » explicite (Ctrl-C ou Ctrl-D = refus, rien n'est modifié) ; s'il relit le fichier et le trouve changé entre-temps, il n'écrit rien et affiche « settings.json non modifié : … » — relance simplement la commande. Il sauvegarde l'ancien fichier en `settings.json.bak-AAAAMMJJ-HHMMSS` (jamais écrasée) avant d'écrire le nouveau de façon atomique, en gardant les hooks des autres outils ; la sauvegarde et le fichier réécrit gardent tous les deux le mode (droits) de l'original. Il crée ensuite `config.toml` s'il est absent (0600, liste blanche vide, jamais écrasé) et le LaunchAgent qui lance le daemon à l'ouverture de ta session macOS, puis affiche « LaunchAgent chargé ».

   Vérifie la ligne « exécutable : … » affichée pendant l'étape LaunchAgent : elle doit pointer vers `~/.local/bin/discord-rich-claude`, pas vers un `.venv` de checkout de développement.

   Les sessions Claude Code déjà ouvertes prennent en compte les nouveaux hooks sans redémarrage (les hooks sont rechargés à chaud depuis les fichiers de settings).
3. Renseigne `~/.config/discord-rich-claude/config.toml` :

   ```toml
   client_id = "ton Application ID"
   inactif_apres_min = 10
   masquer_apres_min = 60
   rotation_s = 20

   [projets]
   autorises = ["/chemin/complet/vers/un/projet"]
   ```

   La config est relue toute seule quand elle change. Sans `client_id`, le daemon journalise une seule erreur (« client_id absent : pas de connexion à Discord ») et n'affiche rien.

   `masquer_apres_min` (défaut 60 min) : la carte disparaît après une heure sans activité, plutôt que de rester coincée sur « Inactif » toute la nuit.

   Dans Discord, active Paramètres > Confidentialité de l'activité > « Partager mon activité ».

## Ce qui s'affiche

- Ligne 1 : l'activité (« Écrit du code », « Lance une commande », « Pilote 3 sous-agents », « Attend ta réponse », « Inactif »…), puis le nom du projet s'il est autorisé, sinon « Projet privé ».
- Ligne 2, en alternance toutes les `rotation_s` secondes : le modèle et les tokens de la session, puis le nombre de sessions et le cumul du jour.
- Le chrono part de l'ouverture de la plus ancienne session encore ouverte.

Les tokens comptés excluent les lectures du cache. Discord limite la mise à jour de la carte à une fois toutes les 15 s : l'activité peut avoir jusqu'à 15 s de retard.

## Confidentialité

- Refus par défaut : un projet absent de `projets.autorises` s'affiche « Projet privé ». La comparaison porte sur le chemin complet.
- Jamais affiché : noms de fichiers, commandes, prompts, branche git, chemins.
- Le daemon ne garde qu'une liste fermée de champs de chaque hook : `session_id`, `cwd`, `hook_event_name`, `tool_name`, `agent_id`, `transcript_path`, et l'en-tête `X-Claude-Pid`. Le reste (texte des prompts, `tool_input`, `last_assistant_message`…) est jeté à la réception et jamais écrit dans le log.

## Commandes

| Commande | Effet |
|---|---|
| `discord-rich-claude status` | Daemon joignable, Discord connecté, nombre de sessions |
| `discord-rich-claude install` | Hooks, config, LaunchAgent (relançable sans risque) |
| `discord-rich-claude uninstall` | Retire nos hooks et le LaunchAgent, garde la config |
| `discord-rich-claude run` | Lance le daemon au premier plan (le LaunchAgent le fait pour toi) |

`uninstall` garde la config, les sauvegardes de `settings.json`, les logs et le dossier d'état ; seul l'outil lui-même s'enlève, avec `uv tool uninstall discord-rich-claude`.

## Mise à jour

```bash
uv tool install --reinstall .
launchctl kickstart -k gui/$(id -u)/local.discord-rich-claude
```

## Dépannage

- Log : `~/Library/Logs/discord-rich-claude/daemon.log` (tourne automatiquement). Plus de détail : `DRC_LOG=DEBUG discord-rich-claude run` après `launchctl bootout gui/$(id -u)/local.discord-rich-claude`. Le LaunchAgent écrit aussi `launchd.out.log` et `launchd.err.log` dans le même dossier. Pour revenir à l'agent après ce `run` manuel : `launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/local.discord-rich-claude.plist`.
- `discord-rich-claude run` lancé à la main pendant que le LaunchAgent tourne déjà affiche « un daemon tourne déjà sur … » et s'arrête : c'est attendu, l'agent occupe déjà le socket.
- La carte n'apparaît jamais après l'ouverture de session : vérifie la ligne « exécutable : … » affichée par `install` (section Installation) — un chemin sous `/Volumes/` ou un `.venv` signale un mauvais binaire enregistré dans le LaunchAgent, à corriger en réinstallant depuis un terminal classique.
- Pas de carte (daemon actif, bon exécutable) : vérifie `client_id`, que Discord desktop tourne, et Paramètres Discord > Confidentialité de l'activité > « Partager mon activité ».
- Le daemon arrêté ne gêne jamais Claude Code : les hooks échouent sans bruit.

## Développement

```bash
uv sync
```

Commandes de tests, conventions de code : voir `CLAUDE.md`.

Spec : `docs/superpowers/specs/2026-09-18-discord-rich-claude-design.md`.

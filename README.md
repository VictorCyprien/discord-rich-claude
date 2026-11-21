# discord-rich-claude

L'app de barre de menus s'appelle Clauderune (`~/Applications/Clauderune.app`) ; la commande reste `discord-rich-claude`.

Affiche sur ton profil Discord une carte « Joue à ClaudeCode » quand une session Claude Code tourne : activité en cours, modèle, tokens, durée. Le nom du projet n'apparaît que pour les dossiers que tu autorises.

Tout reste local : les hooks de Claude Code parlent au daemon par un socket Unix, le daemon parle à Discord par sa connexion locale. Aucune donnée ne part sur le réseau.

## Installation

Prérequis : macOS, la toolchain Rust (`cargo`, installé via [rustup](https://rustup.rs)) et Discord ouvert.

1. Crée l'application Discord : https://discord.com/developers/applications > New Application. Le Developer Portal refuse un nom avec une espace (erreur 50277, testé avec « Claude Code ») : utilise par exemple `ClaudeCode` ou `Claude·Code`. Le titre de la carte est le nom de l'application. Dans Rich Presence > Art Assets, dépose une image carrée sous la clé d'asset `logo`. Copie l'Application ID (General Information).
2. Compile, puis installe :

   ```bash
   cargo build --release
   ./target/release/discord-rich-claude install
   ```

   `install` refuse de tourner avec `sudo`. Il procède en 5 étapes :

   1. **Hooks Claude Code.** Il te montre la différence sur `~/.claude/settings.json` et attend un « o » explicite (Ctrl-C ou Ctrl-D = refus, rien n'est modifié). S'il relit le fichier et le trouve changé entre-temps, il n'écrit rien et affiche « settings.json non modifié : … » : relance simplement la commande. Il sauvegarde l'ancien fichier en `settings.json.bak-AAAAMMJJ-HHMMSS` (jamais écrasée) avant d'écrire le nouveau de façon atomique, en gardant les hooks des autres outils ; la sauvegarde et le fichier réécrit gardent tous les deux le mode (droits) de l'original.
   2. **Configuration.** Il crée `config.toml` s'il est absent (0600, liste blanche vide, jamais écrasé).
   3. **Application.** Il copie le binaire dans `~/Applications/Clauderune.app` (bundle minimal avec son icône, signé ad hoc, sans notarisation) et affiche « exécutable : … ». Si l'ancienne app `Discord Rich Claude.app` est présente, elle est retirée en dernier, une fois le LaunchAgent chargé et le lien posé (voir Migration).
   4. **LaunchAgent.** Il écrit et charge `~/Library/LaunchAgents/local.discord-rich-claude.plist`, qui lance le binaire du bundle à l'ouverture de ta session macOS, puis affiche « LaunchAgent chargé ».
   5. **Commande.** Il pose le lien `~/.local/bin/discord-rich-claude` vers le binaire du bundle. Il refuse, sans rien modifier, si une autre entrée occupe ce chemin. Un lien qui vise l'ancienne app est remplacé (« lien mis à jour »). Pour une migration depuis l'ancienne version Python, lance d'abord `uv tool uninstall discord-rich-claude`.

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

## Barre de menus

Le daemon vit dans la barre de menus macOS : une icône, sans présence dans le Dock. Trois états :

| État | Effet |
|---|---|
| Allumé | Les hooks sont suivis, la carte est affichée (Discord se connecte quand une session compte). |
| En pause | Les hooks sont suivis et les sessions restent à jour, mais la carte est effacée. La connexion à Discord est gardée si elle est déjà ouverte. Au retour à « Allumé », la carte courante est renvoyée aussitôt (dans la limite de 15 s). |
| Arrêté | La connexion à Discord est fermée, les hooks sont répondus `204` et ignorés, rien n'est gardé. Au retour à « Allumé », l'état se reconstruit au fil de l'eau. |

Le menu compte une ligne d'état grisée (« Allumé », « En pause » ou « Arrêté »), puis « Allumer », « Mettre en pause », « Arrêter », un séparateur et « Quitter ». Il compte aussi un sous-menu « Projets » (voir plus bas). Il n'affiche jamais de session ni de projet hors de la liste blanche.

- L'état est toujours « Allumé » au lancement, donc à l'ouverture de session : aucun état n'est mémorisé sur disque.
- « Quitter » ferme le daemon et ne relance rien. Pour le relancer, ouvre l'app « Clauderune » (Spotlight, Launchpad) : sans argument, le binaire fait `run`.
- Un crash n'est pas relancé : le plist n'a pas de `KeepAlive`. Le daemon arrêté ne gêne jamais Claude Code (voir Dépannage).

## Projets

Le sous-menu « Projets » montre les projets de la liste blanche, triés par nom affiché. Chaque projet a un sous-menu : son chemin (grisé), « Renommer… », « Changer le dossier… » et « Retirer… ». « Ajouter un projet… » choisit un dossier puis demande le nom à afficher sur Discord. Les dialogues passent par `osascript`. Le menu réécrit `config.toml` (0600, de façon atomique) ; il refuse si le fichier est invalide ou est un lien symbolique.

Un projet peut porter un alias, affiché sur Discord à la place du nom de son dossier, y compris pour ses sous-dossiers et ses worktrees :

```toml
[projets]
autorises = ["~/Workspace/Perso/discord-rich-claude"]

[projets.alias]
"~/Workspace/Perso/discord-rich-claude" = "DiscordRichClaude"
```

La clé doit désigner un chemin de `autorises` (comparé après résolution) ; sinon la config est invalide et rien n'est affiché. Un alias fait de 1 à 32 caractères, sans espace au début ni à la fin, sans caractère de contrôle.

## Migration

Avant Clauderune, l'app s'appelait `Discord Rich Claude.app`. `install` la retire en dernier, après avoir chargé le LaunchAgent et posé le lien (`ancienne app retirée : …`) et met à jour le lien `~/.local/bin/discord-rich-claude` s'il visait l'ancien binaire. Si l'ancienne app ne peut pas être retirée, `install` le signale (`ancienne app laissée en place : …`) et continue.

## Commandes

| Commande | Effet |
|---|---|
| `discord-rich-claude run` | Lance le daemon et l'icône de la barre de menus au premier plan (ouvrir l'app fait la même chose) |
| `discord-rich-claude install` | Hooks, config, app, LaunchAgent, lien de commande (relançable sans risque) |
| `discord-rich-claude uninstall` | Retire nos hooks, le LaunchAgent, l'app et le lien de commande ; garde la config |
| `discord-rich-claude status` | Daemon joignable, Discord connecté, nombre de sessions, état (Allumé, En pause, Arrêté) |

Il n'y a ni `pause` ni `resume` en ligne de commande : seul le menu change l'état.

## Mise à jour

```bash
cargo build --release
./target/release/discord-rich-claude install
```

`install` remplace le binaire du bundle, puis décharge et recharge le LaunchAgent : le daemon repart aussitôt avec la nouvelle version.

## Désinstallation

```bash
discord-rich-claude uninstall
```

Même confirmation « o » que `install` pour `settings.json`. Elle retire nos hooks, le LaunchAgent, `~/Applications/Clauderune.app` (et `Discord Rich Claude.app` si elle traîne encore) et le lien `~/.local/bin/discord-rich-claude` (seulement s'il pointe vers le binaire du bundle, ancien ou nouveau ; sinon il est laissé en place). Elle garde la config, les sauvegardes de `settings.json`, les logs et le dossier d'état.

## Dépannage

- Log : `~/Library/Logs/discord-rich-claude/daemon.log` (tourne automatiquement). Plus de détail : quitte d'abord le daemon par le menu (« Quitter »), puis lance `DRC_LOG=DEBUG discord-rich-claude run`. Le LaunchAgent écrit aussi `launchd.out.log` et `launchd.err.log` dans le même dossier. Pour revenir au lancement normal, quitte ce `run` (Ctrl-C) puis ouvre l'app « Clauderune ».
- `discord-rich-claude run` lancé à la main pendant que le daemon tourne déjà affiche « un daemon tourne déjà sur … » et s'arrête : c'est attendu, le daemon occupe déjà le socket.
- Pas de carte : vérifie `client_id`, que Discord desktop tourne, l'état dans la barre de menus (« En pause » ou « Arrêté » n'affichent rien) et Paramètres Discord > Confidentialité de l'activité > « Partager mon activité ». `discord-rich-claude status` résume l'essentiel.
- Le daemon arrêté ne gêne jamais Claude Code : les hooks échouent sans bruit.

## Développement

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
DRC_LOG=DEBUG cargo run -- run
```

Conventions de code et de test : voir `CLAUDE.md`.

Spec : `docs/superpowers/specs/2026-09-18-discord-rich-claude-design.md`.

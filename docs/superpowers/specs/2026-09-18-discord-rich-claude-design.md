# Discord Rich Presence pour Claude Code — design

Date : 2026-09-18 · Statut : validé en brainstorming, en attente de relecture

## 1. Objectif

Quand une session Claude Code tourne sur la machine, le profil Discord affiche une carte « Joue à Claude Code » avec l'activité en cours, le modèle, les tokens et la durée. Le nom du projet n'apparaît que pour les projets explicitement autorisés.

Usage personnel, macOS, Discord desktop installé.

## 2. Décisions

| Sujet | Décision | Raison |
|---|---|---|
| Réutiliser un projet existant | Non, on code le nôtre | 5 projets évalués (rar-file/claude-rpc, BrunoJurkovic/claude-code-discord-status, tsanva et Jerit3787/cc-discord-presence, StealthyLabsHQ/claude-rpc) : aucun n'a de liste blanche de projets. Le plus proche (rar-file) a une liste noire et envoie des statistiques à un serveur tiers par défaut. |
| Architecture | Hooks Claude Code + daemon local | Un hook seul ne peut pas tenir la carte : Discord l'efface quand la connexion IPC se ferme. Les hooks donnent un état exact et immédiat. |
| Langage | Python 3.13, uv, asyncio, pypresence | Stack maîtrisée, relisible. Go aurait un binaire unique et moins de RAM, gain négligeable en usage perso. |
| Transport hook → daemon | `curl` sur socket Unix, hooks **synchrones** | Garantit l'ordre des événements (un `Stop` ne peut pas arriver avant le dernier `PreToolUse`). Coût attendu ~10 ms par événement, à mesurer en L1. |
| Nom du projet | Liste blanche, refusé par défaut | Aucun nom de projet client ne peut sortir par erreur. |
| Plusieurs sessions | La carte suit la session la plus récemment active, avec un compteur | |
| Tokens | Sans les lectures du cache | Les lectures du cache (94 % du total mesuré sur une session réelle) grossissent avec la longueur du contexte, pas avec le travail fait. |
| Demande de permission | `PermissionRequest`, pas `Notification` | Plus précis. `notification_type` sort donc de la liste des champs lus. |

## 3. Architecture

```
Claude Code ──hook synchrone: curl --unix-socket──▶ daemon Python ──IPC──▶ Discord
                                                      │
                                           lit les transcripts JSONL
                                           (tokens, modèle)
```

Modules du daemon, chacun testable seul :

| Module | Rôle | Dépendances |
|---|---|---|
| `serveur` | Reçoit les événements sur le socket Unix, filtre les champs, les passe à `sessions` | asyncio (stdlib) |
| `sessions` | État en mémoire de chaque session. Logique pure, aucune entrée-sortie | aucune |
| `tokens` | Lecture incrémentale des transcripts : tokens par session, cumul du jour, modèle | aucune |
| `carte` | Fonction pure : état + config + instant → carte Discord (ou « rien ») | aucune |
| `discord` | Connexion pypresence, reconnexion, envoi regroupé, effacement | pypresence |
| `config` | Lecture et rechargement du TOML | tomllib (stdlib) |
| `installation` | `install` / `uninstall` : hooks dans settings.json, LaunchAgent | stdlib |

Le serveur HTTP est codé à la main sur `asyncio.start_unix_server` (deux routes : `POST /event`, réponse `204` ; `GET /etat`, réponse `200` avec `{"discord": bool, "sessions": int}` pour la commande `status`), sans aiohttp : c'est petit. Corps limité à 16 Mo : un `PreToolUse` sur `Write` transporte tout le contenu du fichier.

Une boucle de contrôle tourne toutes les 30 s : inactivité, sessions mortes, relecture des transcripts, rechargement de la config, rotation de la ligne 2.

## 4. Événements et état

### 4.1 Hooks écoutés

| Hook | Effet sur la session |
|---|---|
| `SessionStart` | Crée la session : dossier, heure de début, transcript |
| `UserPromptSubmit` | Activité du fil principal = « Réfléchit » |
| `PreToolUse` | Activité du fil principal = libellé de l'outil (4.2) |
| `PermissionRequest` | « Attend ta validation » |
| `Stop` | « Attend ta réponse » |
| `SubagentStart` | Ajoute `agent_id` à la liste des sous-agents en cours |
| `SubagentStop` | Retire `agent_id` de la liste |
| `SessionEnd` | Supprime la session |

Tout événement, quel qu'il soit, met à jour l'heure du dernier événement de la session. Une session inconnue est créée au premier événement reçu, pas seulement à `SessionStart` : après un redémarrage du daemon, l'état se reconstruit au fil de l'eau.

Un événement qui porte un `agent_id` vient d'un sous-agent : il compte comme activité mais ne change pas l'activité du fil principal.

### 4.2 Libellés d'outil

Jamais de nom de fichier ni de commande, seulement une catégorie :

```
Edit, Write, NotebookEdit  → « Écrit du code »
Bash                       → « Lance une commande »
Read, Grep, Glob           → « Lit le code »
WebSearch, WebFetch        → « Cherche sur le web »
Agent                      → « Pilote des sous-agents »
tout autre outil, MCP      → « Travaille »
```

### 4.3 Activité affichée d'une session

Par ordre de priorité :

1. Aucun événement (fil principal ou sous-agent) depuis `inactif_apres_min` → « Inactif ».
2. Liste des sous-agents non vide → « Pilote N sous-agents » (« Pilote 1 sous-agent » au singulier).
3. Sinon, l'activité du fil principal.

Quand la liste des sous-agents se vide, la carte revient à l'activité du fil principal, par exemple « Attend ta réponse » si `Stop` est arrivé entre-temps.

### 4.4 Boucle de contrôle (30 s)

- Un sous-agent sans aucun événement depuis `inactif_apres_min` est retiré de la liste (`SubagentStop` perdu).
- Une session dont le processus Claude n'existe plus est supprimée (terminal tué, crash, `SessionEnd` perdu). Le hook envoie le PID du processus Claude dans l'en-tête `X-Claude-Pid: $PPID` ; le daemon teste `os.kill(pid, 0)`.
- Sans PID valide pour une session, pas de purge par PID. Si L1 montre que `$PPID` n'est pas le processus Claude, repli : purge d'une session sans événement depuis 12 h.

### 4.5 Plusieurs sessions

- Session affichée : celle dont le dernier événement est le plus récent.
- Compteur : nombre de sessions vivantes.
- Tokens de la session : ceux de la session affichée.
- Cumul du jour : toutes les sessions, depuis minuit heure locale.
- Zéro session vivante : la carte est effacée et la connexion à Discord fermée.

## 5. La carte

```
JOUE À
Claude Code
[logo]  Écrit du code · DiscordRichClaude       ← details
        Opus 5 · 445 k tokens                    ← state, vue A
        01:23:45 écoulé                          ← timestamps.start

  toutes les rotation_s secondes, state passe à la vue B :

        3 sessions · 8,4 M aujourd'hui           ← state, vue B
```

- Titre : nom de l'application Discord créée par Victor (« Claude Code »). Logo : image de l'application, clé d'asset `logo`.
- `details` : activité (4.3), puis ` · <projet>` si le dossier est autorisé, sinon ` · Projet privé`.
- `state`, vue A : `<modèle> · <tokens session> tokens`.
- `state`, vue B : `<N> session(s) · <cumul du jour> aujourd'hui`.
- Chrono : heure de début de la plus ancienne session vivante. Il ne saute pas quand la carte change de session. Après un redémarrage du daemon, le début d'une session reconstruite est l'heure du premier événement reçu.
- Libellés en français, codés en dur.
- Formats de nombre : `< 1 000` tel quel (`850`), `< 1 000 000` en milliers (`445 k`), au-delà en millions avec une décimale et virgule (`1,2 M`).
- Nom du modèle : `claude-(opus|sonnet|haiku|fable)-X[-Y][-AAAAMMJJ]` → `<Famille> X` ou `<Famille> X.Y` (`claude-opus-5` → `Opus 5`, `claude-haiku-4-5-20251001` → `Haiku 4.5`). Identifiant inconnu : affiché sans le préfixe `claude-`. Modèle absent : vue A sans modèle.

Envoi à Discord : seulement si la carte a changé, et pas plus d'une fois par intervalle minimal. La valeur de cet intervalle dépend de la limite de fréquence de Discord, vérifiée en L1 ; les mises à jour d'une rafale sont regroupées, la dernière gagne.

## 6. Tokens et modèle

- Tokens comptés : `input_tokens + cache_creation_input_tokens + output_tokens`. `cache_read_input_tokens` est exclu.
- Fichiers d'une session : son transcript (`transcript_path`) et `<dossier du transcript>/<session_id>/subagents/*.jsonl`.
- Dédoublonnage : une entrée par couple `(message.id, requestId)` à l'échelle de chaque agrégat (session, jour), y compris entre fichiers différents, en gardant pour chaque champ la valeur maximale. Mesuré sur une session réelle : 100 entrées de sous-agents répétées avec des valeurs différentes. Garder la première occurrence sous-compte.
- Lecture incrémentale : un décalage en octets par fichier, seules les lignes complètes sont lues ; une ligne incomplète en fin de fichier attend le tour suivant. Ligne illisible : ignorée et notée dans le log.
- Cumul du jour : entrées dont le `timestamp` (UTC dans les transcripts) tombe après minuit heure locale, dans tous les `~/.claude/projects/**/*.jsonl` modifiés depuis minuit. Recalculé au démarrage du daemon depuis les transcripts : aucun fichier d'état propre. Remis à zéro à minuit.
- Modèle d'une session : `message.model` de la dernière entrée assistant du transcript principal, en ignorant `<synthetic>`.
- Les transcripts sont écrits en différé par Claude Code : les chiffres peuvent avoir un tour de retard.

## 7. Configuration et confidentialité

### 7.1 Fichier de config

`~/.config/discord-rich-claude/config.toml`, rechargé quand sa date de modification change :

```toml
client_id = "1234567890"      # ID de l'application Discord
inactif_apres_min = 10
rotation_s = 20

[projets]
# Refusé par défaut : seuls ces dossiers affichent leur nom.
autorises = [
  "/Volumes/MAC-Extended/Workspace-MAC/DiscordRichClaude",
]
```

- Fichier absent ou invalide : aucun projet autorisé, le daemon continue et écrit un avertissement.
- `client_id` absent : pas de connexion à Discord, erreur dans le log.

### 7.2 Liste blanche

- Comparaison sur chemins complets résolus, jamais sur le nom du dossier.
- Un `cwd` égal à un chemin autorisé ou situé dessous (sous-dossier, worktree) est autorisé.
- Nom affiché : dernier élément du chemin autorisé, pas du sous-dossier.

### 7.3 Ce que le daemon garde d'un événement

Liste fermée : `session_id`, `cwd`, `hook_event_name`, `tool_name`, `agent_id`, `transcript_path`, et l'en-tête `X-Claude-Pid`. Tout le reste (texte du prompt, `tool_input`, `last_assistant_message`…) est jeté à la réception, jamais gardé ni écrit dans le log.

`transcript_path` n'est utilisé que s'il est sous `~/.claude/projects/` et finit par `.jsonl`.

### 7.4 Jamais sur Discord

Noms de fichiers, commandes, prompts, branche git, chemins. Un nom de projet uniquement via la liste blanche.

### 7.5 Réseau et fichiers

- Aucun accès réseau : ni télémétrie ni vérification de mise à jour. Seule la connexion IPC locale à Discord.
- Socket : `~/.local/state/discord-rich-claude/daemon.sock`, dossier en 0700, socket en 0600. Chemin sans espace, sous la limite de 104 octets de macOS.
- Log : `~/Library/Logs/discord-rich-claude/daemon.log`, via `logging`. Contenu : type d'événement, identifiant de session, erreurs. Jamais de contenu de prompt ni de chemin de fichier édité.

## 8. Installation et démarrage

- `uv tool install .` fournit la commande `discord-rich-claude` : `run`, `install`, `uninstall`, `status`.
- `install` :
  1. calcule l'ajout des hooks (section 4.1), affiche la différence, attend un « o » explicite ;
  2. relit le fichier : s'il a changé pendant la confirmation, n'écrit rien et demande de relancer ; sinon le sauvegarde en `settings.json.bak-AAAAMMJJ-HHMMSS` (mêmes droits que l'original, jamais écrasée) ;
  3. écrit de façon atomique, avec les droits de l'original, en conservant les hooks existants d'autres outils. Idempotent : nos hooks sont reconnus à la présence de `discord-rich-claude` dans la commande ;
  4. crée `~/Library/LaunchAgents/local.discord-rich-claude.plist` (`RunAtLoad`, `KeepAlive`, lance `discord-rich-claude run`) et le charge avec `launchctl bootstrap gui/<uid>` ;
  5. crée la config avec une liste blanche vide si elle n'existe pas.
- `uninstall` : retire seulement nos hooks et le LaunchAgent (`launchctl bootout`). Garde la config.
- `status` : daemon joignable ou non, Discord connecté ou non, sessions vivantes.
- Commande d'un hook :

  ```sh
  curl -s --max-time 1 --unix-socket "$HOME/.local/state/discord-rich-claude/daemon.sock" -H "X-Claude-Pid: $PPID" --data-binary @- http://localhost/event >/dev/null 2>&1 || true
  ```

  Hook synchrone, `timeout: 2`. `PreToolUse` avec le matcher `*`.
- Étape manuelle, documentée dans le README : créer l'application « Claude Code » sur le Developer Portal de Discord, y déposer le logo sous la clé `logo`, copier l'Application ID dans `client_id`.

## 9. Pannes et reprises

| Panne | Comportement |
|---|---|
| Discord fermé ou redémarré | Tant qu'une session est vivante, nouvel essai toutes les 15 s ; la carte courante est envoyée dès la reconnexion |
| Daemon arrêté | `curl` échoue sans bruit, Claude n'est pas gêné ; le LaunchAgent relance le daemon, l'état se reconstruit (4.1) |
| Ligne de transcript illisible | Ignorée, notée dans le log |
| Erreur sur un événement | Notée dans le log, le daemon continue |
| Config invalide | Refus par défaut (7.1) |

Limite connue : après une validation de permission, « Attend ta validation » reste affiché pendant l'exécution de l'outil validé, jusqu'à l'événement suivant. Pas de `PostToolUse` pour la corriger : c'est un événement de plus à chaque outil pour un cas bref.

## 10. Tests

Test d'abord, à chaque étape du plan.

- Unitaires (pytest) :
  - `sessions` : chaque transition de 4.1, priorités de 4.3, sous-agents (premier plan et arrière-plan), inactivité, purge par PID, reconstruction après redémarrage ;
  - `carte` : liste blanche (exact, sous-dossier, homonyme non autorisé, config vide), rotation A/B, formats de nombre, noms de modèle, singulier/pluriel ;
  - `tokens` : doublons à valeurs différentes, ligne incomplète en fin de fichier, ligne illisible, passage de minuit, sous-agents inclus, exclusion du cache lu, `<synthetic>` ignoré ;
  - `config` : absent, invalide, `client_id` manquant, rechargement ;
  - `serveur` : champs hors liste jetés, `transcript_path` hors `~/.claude/projects/` refusé, corps trop gros ;
  - `installation` : settings.json avec des hooks tiers conservés, idempotence, uninstall ne retire que les nôtres.
- Intégration : daemon réel, vrai socket, vrai `curl` avec la commande de hook exacte, client Discord simulé. Discord est un service externe : il est mocké.
- Protocole manuel : vraie carte Discord vue depuis un autre compte ou un téléphone. Parcours : une session, deux sessions, sous-agents en arrière-plan, projet autorisé vs privé, Discord redémarré, daemon tué.

## 11. Lots

| Lot | Contenu | Preuve |
|---|---|---|
| L1 — sonde | Hook temporaire qui journalise les champs de 7.3 et `$PPID`. Vérifie les hypothèses de la section 12. | Journal d'une vraie session avec sous-agents et une demande de permission |
| L2 — cœur | `serveur`, `sessions`, `carte` (sans tokens), `discord`, `config` | La carte suit l'activité en direct sur le profil |
| L3 — tokens | `tokens`, cumul du jour, rotation A/B | Chiffres de la carte égaux à un recomptage indépendant |
| L4 — installation | `install`, `uninstall`, `status`, LaunchAgent, README, CLAUDE.md | Installation sur settings.json réel, redémarrage de la session macOS, carte présente |

## 12. Hypothèses à vérifier en L1

1. Dans un hook synchrone, `$PPID` est le PID du processus Claude Code.
   Résultat (2026-09-18) : confirmée — dans les trois sessions de sonde, `$PPID` était le processus `claude`, le même pour le fil principal et ses sous-agents, et ce pid était mort après `/exit`.
2. Les événements d'un sous-agent portent le `session_id` de la session parente.
   Résultat (2026-09-18) : confirmée — les événements des trois sous-agents Explore portaient le `session_id` de la session parente.
3. `SubagentStop` se déclenche aussi pour les sous-agents en arrière-plan.
   Résultat (2026-09-18) : confirmée — `SubagentStop` part avec `agent_id` et `agent_type` pour chaque sous-agent d'arrière-plan qui se termine. Un sous-agent coupé par `/exit` n'émet rien ; `SessionEnd` le couvre. L'outil qui lance un sous-agent s'appelle bien `Agent`.
4. `PermissionRequest` se déclenche dans le mode de permission habituel de Victor (`auto`) quand une validation est demandée.
   Résultat (2026-09-18) : confirmée — en mode Manual, la demande de validation d'une commande a émis `PermissionRequest` (`tool_name` `Bash`) avant la décision. En mode `auto`, la même commande n'a demandé aucune validation et aucun événement n'est parti, comme attendu.
5. Latence réelle d'un hook synchrone `curl` sur le socket (~10 ms supposé).
   Résultat (2026-09-18) : confirmée — un hook `curl` synchrone sur le socket coûte environ 13 ms (50 appels en 0,65 s ; médiane 13,3 ms pour un petit corps, 21,9 ms pour un `PreToolUse` sur `Write` de 2 Mo).
6. Limite de fréquence de Discord sur la mise à jour d'activité, pour fixer l'intervalle minimal de la section 5.
   Résultat (2026-09-18) : confirmée — la documentation Discord (Setting Rich Presence) limite la mise à jour d'activité à une toutes les 15 secondes ; l'issue discord/discord-api-docs#668 montre qu'un dépassement peut vider la présence au lieu de la mettre en file. Intervalle minimal retenu : 15 s.
7. Emplacement des transcripts de sous-agents : `<dossier du transcript>/<session_id>/subagents/`. Constaté sur une session le 2026-09-18, à confirmer.
   Résultat (2026-09-18) : confirmée — `agent_transcript_path` pointe vers `<dossier du transcript>/<session_id>/subagents/agent-<agent_id>.jsonl` ; fichier présent pour chacun des trois sous-agents Explore.

Constats annexes de la sonde (2026-09-18) :

- Claude Code émet des `SubagentStop` pour des agents internes, sans `SubagentStart` ni `agent_type`, et sans fichier de transcript. Le registre les absorbe : retirer un `agent_id` inconnu ne fait rien.
- `UserPromptSubmit` part aussi quand un sous-agent rend son résultat ou qu'une tâche de fond notifie le fil principal : le fil principal se remet alors au travail, « Réfléchit » est juste.
- Le fil principal émet `Stop` pendant que ses sous-agents d'arrière-plan tournent encore ; la priorité de 4.3 (sous-agents avant fil principal) affiche alors « Pilote N sous-agents ».
- Une session déjà ouverte a rechargé à chaud les hooks ajoutés à `.claude/settings.local.json` et s'est mise à les exécuter. Même comportement attendu pour `~/.claude/settings.json`, à confirmer en Task 11.

Si une hypothèse tombe, le design de la section concernée est revu avant L2.

## 13. Hors périmètre

Windows et Linux, Claude Desktop, coût en dollars, branche git, boutons sur la carte, petites icônes par activité, statistiques historiques (semaine, total), commande pour gérer la liste blanche, distribution publique.

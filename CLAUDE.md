# CLAUDE.md

Un binaire Rust (daemon et barre de menus macOS) qui affiche une Discord Rich Presence pour Claude Code. La spec fait foi : `docs/superpowers/specs/2026-09-18-discord-rich-claude-design.md`.

Le projet reste en français (code, tests, libellés, commits) : ne rien traduire, y compris dans du nouveau code.

## Commandes

- Tests : `cargo test`
- Lint : `cargo clippy --all-targets -- -D warnings` et `cargo fmt --check`
- Daemon au premier plan, logs détaillés : `DRC_LOG=DEBUG cargo run -- run`

## Structure

`src/` (une crate : lib `discord_rich_claude` et binaire `discord-rich-claude`) :

- `chemins` : emplacements disque, commande de hook.
- `sessions` : état des sessions (pur).
- `carte` : construction de la carte (pur).
- `tokens` : lecture des transcripts.
- `config` : `config.toml`, relu quand il change.
- `serveur` : socket Unix, HTTP minimal, filtrage.
- `presence` : Discord, avec un client IPC écrit à la main.
- `journal` : `daemon.log` avec rotation, hook de panique.
- `daemon` : assemblage, états Allumé, En pause, Arrêté.
- `installation` : hooks de `settings.json`, LaunchAgent.
- `bundle` : app `~/Applications/Discord Rich Claude.app`, signature ad hoc, lien de commande.
- `menu` : barre de menus (tray-icon, tao).
- `cli` : ligne de commande.

`tests/` : tests d'intégration (`cli`, `integration`, `serveur`). `tests/commun` : aides partagées (`dossier_court`, `serie`, faux client Discord).

## Règles

- Confidentialité : `serveur.filtrer` ne garde qu'une liste fermée de champs (`session_id`, `cwd`, `hook_event_name`, `tool_name`, `agent_id`, `transcript_path`, l'en-tête `X-Claude-Pid`). N'en ajouter un qu'avec une décision écrite dans la spec. Jamais de prompt, de `tool_input`, de chemin de fichier édité dans un log ou sur Discord.
- Liste blanche des projets : refus par défaut, comparaison de chemins résolus, jamais de nom de dossier seul.
- Aucun accès réseau. Dépendances : liste fermée de crates, serde, serde_json (preserve_order), indexmap, toml, chrono, similar, libc, log, tray-icon, tao, ctrlc ; tempfile en dev. Aucune autre sans décision écrite.
- `sessions` reste sans entrée-sortie ; `carte` aussi, sauf une résolution de chemin mise en cache par dossier (`nom_projet` : le volume qui porte un projet peut être mal monté, un `resolve()` non mis en cache bloquerait la boucle carte à 1 s).
- Discord est mocké dans les tests (faux client, faux Discord IPC), le socket et `curl` sont réels.
- Libellés en français, ceux de la spec, codés en dur (jamais improvisés).
- Sous macOS, un chemin de socket Unix est limité à 104 octets : les tests de socket utilisent `commun::dossier_court()`.
- Les tests qui démarrent un serveur sont sérialisés par `serie()` (`tests/commun`) : l'umask est global au processus.
- Les tests ne touchent jamais les vrais chemins utilisateur (`~/.claude`, `~/.config`, `~/Library`, `~/Applications`, `~/.local/bin`) : `installation`, `bundle` et `chemins` sont testés avec des chemins temporaires (`tempfile`), jamais les emplacements réels. Aucun test n'appelle le vrai `launchctl` ni le vrai `codesign` : `executer` est injectable (`installer_launch_agent`, `desinstaller_launch_agent`, `installer_bundle`) et remplacé par un faux dans les tests.
- Aucun test n'appelle `principal` sans argument : sans argument, il lance le daemon.
- Noms des tests : un test porté depuis pytest garde `test_<nom pytest>`, un test nouveau n'a pas de préfixe `test_`.
- La confirmation de `install`/`uninstall` sur `~/.claude/settings.json` (le « o » explicite) revient à Victor : ne jamais y répondre à sa place, ni la simuler, ni contourner `confirmer_terminal` / `confirmer_depuis` (`cli.rs`).
- La sauvegarde de `settings.json` et le fichier réécrit gardent tous les deux le mode (droits) de l'original, jamais un mode plus large même sous umask. `config.toml`, s'il est absent, est créé en 0600 avec une liste blanche vide, jamais écrasé s'il existe déjà.

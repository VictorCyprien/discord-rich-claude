# Réécriture Rust et widget de barre de menus — plan d'implémentation

> **Pour les agents :** sous-skill requis : superpowers:subagent-driven-development (recommandé) ou superpowers:executing-plans, tâche par tâche. Les étapes sont des cases à cocher (`- [ ]`).

**Objectif :** remplacer le daemon Python par un binaire Rust unique, au comportement identique. Le binaire porte une icône de barre de menus (Allumer / Mettre en pause / Arrêter / Quitter), s'installe en `.app`, et démarre au login.

**Architecture :** une crate Rust `rust/discord-rich-claude` (lib + bin) reprend les modules Python un pour un, avec des threads et sans async. Une seconde crate, `rust/oracle`, est générée par `rust-anything bridge`. Elle rejoue contre la lib Rust le comportement enregistré des fonctions pures Python : c'est la preuve d'équivalence. Les parties à entrées-sorties (socket, Discord, launchctl, UI) sont prouvées par les tests pytest portés en Rust. Le Python reste jusqu'à la parité, puis il est supprimé (tâche 13).

**Stack :** Rust édition 2024 (rustc 1.100), cible `aarch64-apple-darwin`, `discord-presence` 3.2, `tray-icon` + `tao`, `rust-anything` (MCP et CLI déjà installés).

**Spec :** `docs/superpowers/specs/2026-09-18-discord-rich-claude-design.md`, sections 1 à 13 pour le comportement et section 14 pour ce qui change. La spec fait foi.

## Contraintes globales

- Tout en français : identifiants, tests, libellés, messages de log, commits (`CLAUDE.md`). Rien n'est traduit.
- Pas de tokio, ni d'async. Threads et `std::sync::mpsc` (ou `crossbeam-channel` si la crate Discord l'apporte déjà).
- Liste fermée de crates. Exécution : `serde` (derive), `serde_json` (feature `preserve_order`), `indexmap`, `toml`, `chrono`, `similar`, `libc`, `log`, `discord-presence` 3.2 (feature Cargo `discord`), `tray-icon`, `tao`, `ctrlc` avec la feature `termination` (feature Cargo `menu`). Tests : `tempfile`. Toute autre crate demande l'accord de Victor.
- Features de la lib : `default = ["menu", "discord"]`. `rust/oracle` dépend de la lib avec `default-features = false`.
- Aucun accès réseau.
- Confidentialité inchangée (spec 7.3 à 7.5). On garde seulement `session_id`, `cwd`, `hook_event_name`, `tool_name`, `agent_id`, `transcript_path` et l'en-tête `X-Claude-Pid`. Jamais de prompt, de `tool_input` ni de chemin de fichier édité dans un log, sur Discord ou dans le menu. Là où Python logge `type(e).__name__`, Rust logge le genre de l'erreur (`io::ErrorKind` ou un `&'static str`), jamais son `Display`.
- Libellés codés en dur, copiés à l'identique depuis le Python (`sessions.py`, `carte.py`, `cli.py`) et la spec 14.2/14.3. Jamais improvisés.
- Constantes copiées du Python : `INTERVALLE_MIN_S = 15`, `REESSAI_S = 15`, `DELAI_POIGNEE_MAIN_S = 5`, `PERIODE_CONTROLE_S = 30`, `PERIODE_CARTE_S = 1`, `TAILLE_MAX = 16 Mio`, `DELAI_S = 2`, `PID_MAX = 2^31`, `LIMITE_DISCORD = 128` (en caractères), log `1_000_000` octets × 3 sauvegardes, `bootstrap` : 3 essais, 0,5 s, codes transitoires 5 et 37.
- Les tests ne touchent jamais les vrais chemins (`~/.claude`, `~/.config`, `~/Library`, `~/Applications`). Chemins temporaires seulement. Aucun test n'appelle le vrai `launchctl` ni `codesign` : `executer` est injecté.
- Socket en test : chemin sous `/tmp` (limite macOS de 104 octets). Helper `dossier_court()` dans `tests/commun/mod.rs` : `tempfile::Builder::new().prefix("drc-").tempdir_in("/tmp")`.
- Le « o » de `install`/`uninstall` revient à Victor : aucun agent ne lance `install` ou `uninstall` sur les vrais chemins, ni ne simule la confirmation.
- Droits : la sauvegarde et la réécriture de `settings.json` gardent le mode de l'original. `config.toml` est créé en 0600, de façon exclusive, jamais écrasé. Dossier d'état en 0700, socket en 0600.
- Python `max(..., key=…)` rend le **premier** maximum. `Iterator::max_by` rend le **dernier**. Partout où Python fait `max`, reproduire « premier dans l'ordre d'insertion ». Les tables de sessions et de sous-agents sont des `IndexMap`. On retire avec `shift_remove` pour garder l'ordre.
- Python `//` et `%` arrondissent vers −∞ : utiliser `div_euclid`/`rem_euclid` ou `floor`.
- Commandes (depuis la racine du dépôt) :
  - tests Rust : `cargo test --manifest-path rust/discord-rich-claude/Cargo.toml`
  - lint : `cargo clippy --manifest-path rust/discord-rich-claude/Cargo.toml --all-targets -- -D warnings` et `cargo fmt --manifest-path rust/discord-rich-claude/Cargo.toml --check`
  - oracle : `rust-anything build rust/oracle && rust-anything diff rust/oracle`
  - Python (doit rester vert jusqu'à la tâche 13) : `uv run pytest -q`
- `PATH` doit contenir `$HOME/.cargo/bin`.
- Chaque commit finit par la ligne d'attribution en vigueur dans la session.

### Écarts connus, acceptés d'avance (`rust/oracle/known-diffs.jsonl`)

Ce sont les seuls cas de `diff` qu'on peut déclarer comme écarts. Tout autre écart se corrige dans le Rust.

1. `nom_modele` : le `\d` Python accepte tous les chiffres Unicode (catégorie Nd). Rust n'accepte que l'ASCII. Le `$` Python accepte un `\n` final : Rust le reproduit.
2. `extraire` : un `message.id`, `uuid` ou `requestId` qui n'est ni texte ni entier (flottant, liste, objet) écarte l'entrée en Rust. Un compteur de tokens hors `i64`, `Infinity` ou `NaN` écarte aussi l'entrée.
3. `_horodatage` : formats ISO 8601 rares (forme basique `20260918T100000`, semaines) → `None` en Rust. Formats acceptés : RFC 3339, `AAAA-MM-JJTHH:MM:SS[.f]` et `AAAA-MM-JJ HH:MM:SS[.f]` sans fuseau (heure locale), `AAAA-MM-JJ`.
4. `filtrer` : un corps JSON imbriqué à plus de 128 niveaux est rejeté (limite de `serde_json`).
5. `charger` (config) : une boucle de liens dans un chemin autorisé rend la config invalide (comportement de Python 3.12).

## Points de vigilance de la revue

Cinq cas que la spec implique sans qu'aucun test pytest les couvre, du plus probable au moins probable. Chacun a son test dans la tâche qui possède le code.

1. **Nom de projet ou activité non ASCII proche de 128 caractères.** Tronquer par octets ferait paniquer le daemon (coupure au milieu d'un caractère). Attendu : coupe à 127 caractères + `…`, sans panique. Test dans la tâche 5.
2. **Ordre des clés de `settings.json`.** Sans `preserve_order`, `install` réécrirait le fichier de Victor dans l'ordre alphabétique : diff énorme, et il refuserait. Attendu : les clés gardent leur ordre. Test dans la tâche 10.
3. **Deuxième lancement (`run` à la main alors que le LaunchAgent tourne).** Attendu : `un daemon tourne déjà`, code 1, **aucune** seconde icône dans la barre de menus. Le socket est lié avant de créer l'UI. Test dans la tâche 11.
4. **`install` lancé depuis `cargo run` ou depuis le bundle lui-même.** Attendu : le plist pointe toujours vers le binaire **du bundle**. Une copie du binaire sur lui-même ne casse rien. Tests dans la tâche 10.
5. **Pause puis reprise rapide.** Attendu : l'effacement compte comme un envoi pour la limite de 15 s. La carte revient dès que 15 s sont écoulées depuis l'effacement, ni avant ni jamais. Test dans la tâche 8.

---

## Structure des fichiers

```
rust/discord-rich-claude/
  Cargo.toml               features menu, discord ; [profile.release] opt-level="s", lto=true, codegen-units=1, strip=true
  src/lib.rs               déclare les modules
  src/main.rs              std::process::exit(cli::principal(std::env::args().skip(1).collect()))
  src/chemins.rs           Chemins, LABEL, COMMANDE_HOOK, resoudre()
  src/sessions.rs          Evenement, Session, Registre (pur, sauf pid_vivant)
  src/config.rs            Config, charger, ConfigSurveillee, creer_config_par_defaut
  src/carte.rs             Carte, Stats, construire_carte (pur, sauf le cache de nom_projet)
  src/tokens.rs            Fichier, SuiviTokens, extraire, modele_de, minuit_local
  src/serveur.rs           filtrer, HTTP sur socket Unix, interroger_etat
  src/presence.rs          trait ClientDiscord, Presence ; client réel sous feature discord
  src/journal.rs           logger fichier avec rotation, DRC_LOG
  src/daemon.rs            Mode, Daemon, EnMarche (assemblage, threads)
  src/installation.rs      hooks, modifier_settings, plist, launchctl
  src/bundle.rs            .app, Info.plist, codesign
  src/menu.rs              entrées, icônes (pur) ; boucle tray-icon/tao sous feature menu
  src/cli.rs               run / install / uninstall / status
  examples/sonde_discord.rs
  tests/commun/mod.rs      dossier_court(), FauxClient, envoyer_hook()
  tests/integration.rs     bout en bout : vrai socket, vrai curl
rust/oracle/               généré par rust-anything bridge ; src/port.rs = adaptateurs vers la lib
```

Les tests unitaires d'un module vivent dans son `#[cfg(test)] mod tests`. Chaque test pytest porté garde **son nom** et **ses assertions**.

---

### Task 1: squelette de la crate et sonde Discord

**Fichiers :**
- Créer : `rust/discord-rich-claude/Cargo.toml`, `src/lib.rs`, `src/main.rs`, `examples/sonde_discord.rs`
- Modifier : `.gitignore` (ajouter `rust/*/target/`)

**Interfaces :**
- Produit : la crate qui compile, les features `menu` et `discord`, le profil release.

- [ ] **Étape 1 :** créer la crate (`[lib] name = "discord_rich_claude"`, `[[bin]] name = "discord-rich-claude"`) avec les dépendances de la liste fermée, optionnelles derrière leur feature. `main.rs` imprime `discord-rich-claude : en cours de portage` sur stderr et rend 1.
- [ ] **Étape 2 :** `cargo build --manifest-path rust/discord-rich-claude/Cargo.toml` → compile sans warning.
- [ ] **Étape 3 :** écrire `examples/sonde_discord.rs` (feature `discord`). Elle lit `client_id` dans `~/.config/discord-rich-claude/config.toml` (jamais affiché), se connecte, attend `Ready` 5 s au plus, puis enchaîne ces étapes en imprimant le résultat de chacune : `set_activity` (details `Sonde Rust`, state `essai`, start = maintenant, large_image `logo`, large_text `Claude Code`), attente de 20 s, `clear_activity`, attente de 16 s, second `set_activity`, puis 60 s pendant lesquelles les événements `Connected`/`Disconnected`/`Ready` s'impriment (fenêtre pour quitter et relancer Discord), et enfin `shutdown`.
- [ ] **Étape 4 :** `cargo build --example sonde_discord` → compile. **Ne pas la lancer** : elle touche le vrai profil Discord.
- [ ] **Étape 5 :** commit `feat(rust): squelette de la crate et sonde Discord`.
- [ ] **Étape 6 (Victor, manuel) :** le contrôleur demande à Victor de lancer `! cargo run --manifest-path rust/discord-rich-claude/Cargo.toml --example sonde_discord`, de quitter et relancer Discord pendant la fenêtre de 60 s, et de dire ce qu'il a vu. Selon le verdict, une ligne s'ajoute à la spec 14.1 : « Spike du <date> : crate validée » ou « Spike du <date> : IPC à la main ». Ensuite, la tâche 8 (étape « client réel ») suit le verdict.

---

### Task 2: oracle rust-anything

**Fichiers :**
- Créer : `rust/oracle/` (généré), `rust/oracle/cas-spec.jsonl`

**Interfaces :**
- Produit : `rust/oracle/src/port.rs`, où chaque fonction portée rend d'abord `Err("non porté")`. Les tâches 3 à 7 et 10 les remplacent par des appels à la lib.

- [ ] **Étape 1 :** générer le pont. Sont **gardées en Python** : les fonctions à effets de bord, et celles qui prennent une classe en argument ou dont le comportement change (`generer_plist`, spec 14.4).

```bash
rust-anything bridge . src/discord_rich_claude/ -o rust/oracle \
  --keep "lancer: lance le daemon" --keep "installer: modifie ~/.claude" --keep "desinstaller: modifie ~/.claude" \
  --keep "statut: socket réel" --keep "main: CLI" --keep "configurer_logs: écrit dans ~/Library" \
  --keep "creer_config_par_defaut: écrit un fichier" --keep "interroger_etat: socket réel" \
  --keep "pid_vivant: dépend des processus" --keep "generer_plist: KeepAlive retiré (spec 14.4)" \
  --keep "construire_carte: prend un Registre" --keep "activite_affichee: prend une Session"
```

- [ ] **Étape 2 :** vérifier que `port.rs` ne contient aucune des fonctions gardées (`grep -E "fn (lancer|installer|desinstaller|statut|main|configurer_logs|creer_config_par_defaut|interroger_etat|pid_vivant|generer_plist)\b" rust/oracle/src/port.rs` → rien). Sinon, ne pas lancer `record`.
- [ ] **Étape 3 :** ajouter à `Cargo.toml` de l'oracle `discord-rich-claude = { path = "../discord-rich-claude", default-features = false }`.
- [ ] **Étape 4 :** écrire `cas-spec.jsonl` avec les exemples de la spec, une ligne `{"fn": …, "args": […]}` par cas :
  - `formater_nombre` : 0, 850, 999, 1000, 445000, 999999, 1000000, 1234567, 8400000 ;
  - `nom_modele` : `claude-opus-5`, `claude-haiku-4-5-20251001`, `claude-sonnet-5-5`, `claude-fable-5-2`, `claude-opus-5\n`, `gpt-4`, `claude-inconnu-3`, `""`, `null` ;
  - `est_vue_b` : (0, 20), (19.9, 20), (20, 20), (39.9, 20), (40, 20) ;
  - `libelle_outil` : chaque outil de la spec 4.2, `mcp__x__y`, `null` ;
  - `filtrer` : un corps par cas de `test_filtrer_*` de `tests/test_serveur.py`.
- [ ] **Étape 5 :** enregistrer, avec un `HOME` temporaire comme garde-fou :

```bash
H=$(mktemp -d); UV_CACHE_DIR="$HOME/.cache/uv" UV_OFFLINE=1 HOME="$H" \
  rust-anything record rust/oracle --from-tests --inputs rust/oracle/cas-spec.jsonl -- uv run pytest -q
```

  Attendu : un nombre de cas enregistrés par fonction, et `uv run pytest -q` toujours vert.
- [ ] **Étape 6 :** `rust-anything build rust/oracle && rust-anything diff rust/oracle` → compile. Le diff échoue sur tout (« non porté ») : c'est l'état de départ.
- [ ] **Étape 7 :** commit `chore(rust): oracle rust-anything et cas enregistrés`.

---

### Task 3: `chemins` et `sessions`

**Fichiers :**
- Créer : `src/chemins.rs`, `src/sessions.rs`
- Modifier : `rust/oracle/src/port.rs` (`libelle_outil`, `fichiers_de_session` attendra la tâche 6)

**Interfaces :**
- Produit (`chemins`) :
  - `pub const LABEL: &str = "local.discord-rich-claude";`
  - `pub const COMMANDE_HOOK: &str`, copiée à l'octet près de `chemins.py`
  - `pub struct Chemins { pub home, pub dossier_etat, pub socket, pub config, pub dossier_logs, pub projets_claude, pub settings_claude, pub launch_agent, pub applications: PathBuf }`. `applications` = `~/Applications`, les autres comme `chemins.py`.
  - `impl Chemins { pub fn depuis_home(home: &Path) -> Chemins; pub fn reels() -> Option<Chemins> }` (`reels` lit `HOME`)
  - `pub fn resoudre(p: &Path, strict: bool) -> io::Result<PathBuf>` : équivalent de `Path.resolve(strict)`. Suit les liens composant par composant, traite `..` après résolution, et rend une erreur au-delà de 40 liens. En mode non strict, la partie qui n'existe pas est recollée telle quelle.
- Produit (`sessions`) :
  - constantes `REFLECHIT`, `ATTEND_REPONSE`, `ATTEND_VALIDATION`, `INACTIF`, `TRAVAILLE`
  - `pub fn libelle_outil(tool_name: Option<&str>) -> &'static str`
  - `#[derive(Clone, Debug, PartialEq)] pub struct Evenement { pub nom: String, pub session_id: String, pub recu_a: f64, pub cwd: Option<String>, pub tool_name: Option<String>, pub agent_id: Option<String>, pub transcript_path: Option<String>, pub pid: Option<i32> }`
  - `#[derive(Clone, Debug, PartialEq)] pub struct Session { pub session_id: String, pub debut: f64, pub dernier_evenement: f64, pub activite: String, pub cwd: Option<String>, pub transcript_path: Option<String>, pub pid: Option<i32>, pub sous_agents: IndexMap<String, f64> }`
  - `pub fn activite_affichee(s: &Session, maintenant: f64, inactif_apres_s: f64) -> String`
  - `pub fn pid_vivant(pid: i32) -> bool` : `libc::kill(pid, 0)`. `ESRCH` → faux, `EPERM` → vrai.
  - `#[derive(Default)] pub struct Registre { pub sessions: IndexMap<String, Session> }` avec `appliquer(&mut self, ev: &Evenement)`, `purger(&mut self, maintenant: f64, inactif_apres_s: f64, pid_vivant: impl Fn(i32) -> bool)`, `session_affichee(&self) -> Option<&Session>` (premier maximum), `debut_plus_ancien(&self) -> Option<f64>`

- [ ] **Étape 1 :** porter les 16 tests de `tests/test_sessions.py` (mêmes noms, mêmes assertions), plus des tests de `resoudre` : lien absolu, lien relatif, `..` après un lien, chemin absent en non strict (recollé), chemin absent en strict (`NotFound`), boucle de liens (`Err`).
- [ ] **Étape 2 :** `cargo test` → échec à la compilation (types absents).
- [ ] **Étape 3 :** implémenter `chemins.rs` et `sessions.rs` selon les interfaces ci-dessus, en suivant `sessions.py` ligne à ligne.
- [ ] **Étape 4 :** `cargo test` → tout passe. Adaptateur `libelle_outil` dans `port.rs`, puis `rust-anything build rust/oracle && rust-anything diff rust/oracle` → `libelle_outil` N/N.
- [ ] **Étape 5 :** commit `feat(rust): sessions et chemins`.

---

### Task 4: `config`

**Fichiers :**
- Créer : `src/config.rs`
- Modifier : `rust/oracle/src/port.rs` (`charger`)

**Interfaces :**
- Consomme : `chemins::resoudre`.
- Produit :
  - `#[derive(Clone, Debug, PartialEq)] pub struct Config { pub client_id: Option<String>, pub inactif_apres_min: f64, pub masquer_apres_min: f64, pub rotation_s: f64, pub projets_autorises: Vec<PathBuf> }`, avec `Default` = (None, 10, 60, 20, [])
  - `pub fn charger(chemin: &Path) -> Config`
  - `pub struct ConfigSurveillee { pub chemin: PathBuf, pub valeur: Config, /* mtime en ns, privé */ }`, avec `new(chemin: PathBuf) -> Self` et `recharger_si_change(&mut self) -> bool`
  - `pub const CONFIG_PAR_DEFAUT: &str`, copiée à l'octet près
  - `pub fn creer_config_par_defaut(chemin: &Path) -> io::Result<bool>` : `create_new` + `mode(0o600)`

- [ ] **Étape 1 :** porter les 34 tests de `tests/test_config.py`.
- [ ] **Étape 2 :** `cargo test config` → échec.
- [ ] **Étape 3 :** implémenter en suivant `config.py`. Règles de validation : `client_id` texte ou entier (booléen refusé, entier → texte décimal, `""` → `None`) ; `[projets]` table ; `autorises` liste de textes ; `~` développé avec `HOME` ; chemin relatif refusé ; chemins résolus avec `resoudre(_, false)`. Bornes : `inactif_apres_min` et `masquer_apres_min` dans [1, 1440], `rotation_s` dans [5, 3600], entier ou flottant, booléen refusé, borne testée avant la finitude. Les messages de log reprennent ceux de `config.py`.
- [ ] **Étape 4 :** `cargo test config` → passe. Adaptateur `charger` (sortie sérialisée comme le `Config` Python : chemins en texte), puis diff → `charger` N/N, ou écart n° 5.
- [ ] **Étape 5 :** commit `feat(rust): config`.

---

### Task 5: `carte`

**Fichiers :**
- Créer : `src/carte.rs`
- Modifier : `rust/oracle/src/port.rs` (`formater_nombre`, `nom_modele`, `nom_projet`, `est_vue_b`)

**Interfaces :**
- Consomme : `Registre`, `Session`, `activite_affichee` (tâche 3) ; `Config` (tâche 4) ; `chemins::resoudre`.
- Produit :
  - `pub const PROJET_PRIVE: &str = "Projet privé"; pub const LIMITE_DISCORD: usize = 128;`
  - `#[derive(Clone, Debug, PartialEq)] pub struct Carte { pub details: String, pub state: String, pub debut: i64, pub large_image: &'static str, pub large_text: &'static str }` (`"logo"`, `"Claude Code"`)
  - `#[derive(Clone, Debug, Default, PartialEq)] pub struct Stats { pub modele: Option<String>, pub tokens_session: i64, pub tokens_jour: i64 }`
  - `pub fn formater_nombre(n: i64) -> String`
  - `pub fn nom_modele(model_id: Option<&str>) -> Option<String>` : parseur écrit à la main, sans crate regex, équivalent à `^claude-(opus|sonnet|haiku|fable)-(\d+)(?:-(\d{1,2}))?(?:-\d{8})?$` avec la sémantique Python du `$` (écart n° 1)
  - `pub fn nom_projet(cwd: Option<&str>, autorises: &[PathBuf]) -> String` : résolution de `cwd` mise en cache par dossier (`static` `Mutex<HashMap<String, Option<PathBuf>>>`, 128 entrées au plus, cache vidé quand il est plein)
  - `pub fn tronquer(texte: &str) -> String` : en caractères (`chars()`), jamais en octets
  - `pub fn est_vue_b(maintenant: f64, rotation_s: f64) -> bool` : `(maintenant / rotation_s).floor()`, puis `rem_euclid(2) == 1`
  - `pub fn construire_carte(registre: &Registre, config: &Config, stats: &Stats, maintenant: f64, vue_b: bool) -> Option<Carte>`

- [ ] **Étape 1 :** porter les 22 tests de `tests/test_carte.py`, plus le point de vigilance n° 1 :

```rust
#[test]
fn tronquer_coupe_en_caracteres_sans_paniquer() {
    let texte = "é".repeat(200);
    let t = tronquer(&texte);
    assert_eq!(t.chars().count(), 128);
    assert!(t.ends_with('…'));
    assert_eq!(tronquer(&"🦀".repeat(128)), "🦀".repeat(128));
}
```

- [ ] **Étape 2 :** `cargo test carte` → échec.
- [ ] **Étape 3 :** implémenter en suivant `carte.py`. Le masquage se calcule sur les sessions qui comptent, jamais avec `session_affichee()`. Égalités : premier maximum.
- [ ] **Étape 4 :** `cargo test carte` → passe. Adaptateurs, puis diff → les quatre fonctions N/N, sauf l'écart n° 1. L'arrondi `{:.1}` de `formater_nombre` doit égaler celui de Python : le diff tranche.
- [ ] **Étape 5 :** commit `feat(rust): carte`.

---

### Task 6: `tokens`

**Fichiers :**
- Créer : `src/tokens.rs`
- Modifier : `rust/oracle/src/port.rs` (`minuit_local`, `fichiers_de_session`, `extraire`, `modele_de`)

**Interfaces :**
- Produit :
  - `pub const CHAMPS_COMPTES: [&str; 3] = ["input_tokens", "cache_creation_input_tokens", "output_tokens"];`
  - `pub type Cle = (String, String);`
  - `pub fn minuit_local(maintenant: f64) -> f64` : `chrono::Local`, minuit du jour local. Le décalage horaire est celui de minuit, pas celui de maintenant.
  - `pub fn fichiers_de_session(transcript: &Path) -> Vec<PathBuf>` : `[transcript, triés(<transcript sans .jsonl>/subagents/*.jsonl)]`
  - `pub fn extraire(obj: &serde_json::Value) -> Option<(Cle, Option<f64>, [i64; 3])>` : vérité Python pour `id`/`uuid`/`requestId` (vide, 0, null → faux). Pour les compteurs, l'équivalent de `int(x or 0)` : entier, flottant tronqué, booléen (0 ou 1), texte numérique (espaces et `_` admis). Toute autre forme → `None`. Écart n° 2.
  - `pub fn modele_de(obj: &serde_json::Value) -> Option<String>`
  - `pub struct SuiviTokens`, avec `new(racine: PathBuf)`, `fichiers_suivis(&self) -> HashSet<PathBuf>`, `rafraichir(&mut self, transcripts: &[PathBuf], minuit: f64)`, `tokens_session(&self, transcript: &Path) -> i64`, `tokens_jour(&self, minuit: f64) -> i64` et `modele(&self, transcript: &Path) -> Option<String>`

- [ ] **Étape 1 :** porter les 18 tests de `tests/test_tokens.py`.
- [ ] **Étape 2 :** `cargo test tokens` → échec.
- [ ] **Étape 3 :** implémenter en suivant `tokens.py`. Lecture par lignes terminées par `\n`, avec un décalage en octets ; une ligne incomplète attend. Filtre rapide `"usage"`/`"model"` avant le parsing JSON. Fichier raccourci : on repart de zéro. Parcours récursif de `racine` écrit à la main, qui **ne suit pas** les dossiers liés et garde les fichiers dont le nom finit par `.jsonl`. Jamais tout le fichier en mémoire.
- [ ] **Étape 4 :** `cargo test tokens` → passe. Adaptateurs, puis diff → N/N, hors écarts n° 2 et 3.
- [ ] **Étape 5 :** commit `feat(rust): tokens`.

---

### Task 7: `serveur`

**Fichiers :**
- Créer : `src/serveur.rs`, `tests/commun/mod.rs` (`dossier_court`)
- Modifier : `rust/oracle/src/port.rs` (`filtrer`, `transcript_autorise`)

**Interfaces :**
- Consomme : `Evenement` (tâche 3), `chemins::resoudre`.
- Produit :
  - `pub fn transcript_autorise(chemin: &str, racine: &Path) -> bool`
  - `pub fn filtrer(corps: &[u8], pid_brut: Option<&str>, recu_a: f64, projets_claude: &Path) -> Option<Evenement>`
  - `pub trait Gestionnaire: Send + Sync { fn sur_evenement(&self, ev: Evenement) -> Result<(), &'static str>; fn etat(&self) -> serde_json::Value; }`
  - `pub enum ErreurDemarrage { DejaLance(PathBuf), Io(io::Error) }`
  - `pub fn demarrer_serveur(chemin_socket: &Path, gestionnaire: Arc<dyn Gestionnaire>, projets_claude: PathBuf) -> Result<Serveur, ErreurDemarrage>`
  - `impl Serveur { pub fn arreter(self) }` : arrête la boucle `accept` et attend son thread. Ne supprime pas le socket.
  - `pub fn interroger_etat(chemin_socket: &Path, delai: Duration) -> Option<serde_json::Value>`

- [ ] **Étape 1 :** porter les 26 tests de `tests/test_serveur.py`. Les tests de socket utilisent `dossier_court()`.
- [ ] **Étape 2 :** `cargo test serveur` → échec.
- [ ] **Étape 3 :** implémenter en suivant `serveur.py` :
  - un thread par connexion ;
  - en-tête limité à 64 Kio, puis le corps ; chacun doit arriver dans les 2 s, une échéance **totale** recalculée à chaque lecture, pas un délai par lecture ;
  - réponses à l'octet près du format `_repondre` ;
  - `Expect: 100-continue` ;
  - codes 200, 204, 404, 411, 413, 500 ;
  - erreur du gestionnaire → 500, avec un log qui contient le nom de l'événement et le genre de l'erreur ;
  - dossier d'état : `lstat`, refusé si c'est un lien, si ce n'est pas un dossier ou si un autre uid le possède ; puis `chmod 0700` ;
  - ancien socket : un lien ou un fichier est supprimé ; un socket qui répond → `DejaLance` ; un socket qui ne répond pas est supprimé ;
  - `bind` sous `umask(0o177)`, puis `chmod 0600`.
- [ ] **Étape 4 :** `cargo test serveur` → passe. Adaptateurs, puis diff → `filtrer` et `transcript_autorise` N/N, hors écart n° 4.
- [ ] **Étape 5 :** commit `feat(rust): serveur sur socket Unix`.

---

### Task 8: `presence`

**Fichiers :**
- Créer : `src/presence.rs`

**Interfaces :**
- Consomme : `Carte` (tâche 5).
- Produit :
  - `pub struct ErreurDiscord { pub genre: &'static str }`
  - `pub trait ClientDiscord: Send { fn connecter(&mut self) -> Result<(), ErreurDiscord>; fn publier(&mut self, carte: &Carte) -> Result<(), ErreurDiscord>; fn effacer(&mut self) -> Result<(), ErreurDiscord>; fn fermer(&mut self) -> Result<(), ErreurDiscord>; }`
  - `pub type Fabrique = Box<dyn Fn(&str) -> Box<dyn ClientDiscord> + Send>;`
  - `pub struct Presence`, avec `new(fabrique: Fabrique, intervalle_min_s: f64, reessai_s: f64, horloge: Box<dyn Fn() -> f64 + Send>) -> Self`, `connecte(&self) -> bool`, `soumettre(&mut self, carte: Option<&Carte>, client_id: Option<&str>)`, `effacer_carte(&mut self)` et `fermer(&mut self)`
  - sous `#[cfg(feature = "discord")]` : `pub fn fabrique_reelle() -> Fabrique`

- [ ] **Étape 1 :** porter les tests de `tests/test_presence.py`, sauf les quatre propres à pypresence (`test_client_pypresence_ferme_sans_fermer_la_boucle`, `test_connecter_sans_reponse_expire_et_ferme_le_socket`, `test_connecter_id_invalide_leve_et_ferme_le_socket`, et la partie pypresence de `test_fermeture_echouee_loggee_en_warning`). Ajouter les tests de la pause (spec 14.2, point de vigilance n° 5) :

```rust
#[test]
fn pause_efface_garde_la_connexion_et_la_reprise_attend_15_s() {
    // horloge = 100 : carte publiée ; effacer_carte() à t=100 → journal ["connecter", carte, "effacer"], connecte() == true
    // t=105 : soumettre(même carte) → rien de publié
    // t=115 : soumettre(même carte) → carte republiée
}
#[test]
fn effacer_carte_sans_connexion_ne_fait_rien() { /* journal vide, connecte() == false */ }
#[test]
fn effacer_carte_en_echec_abandonne_la_connexion() { /* effacer lève → connecte() == false, nouvel essai après 15 s */ }
```

- [ ] **Étape 2 :** `cargo test presence` → échec.
- [ ] **Étape 3 :** implémenter `Presence` en suivant `presence.py` (un seul WARNING par genre d'échec répété, puis DEBUG). `effacer_carte` : si un client est connecté, `effacer()`, puis `derniere = None` et `dernier_envoi = maintenant`. En cas d'échec, même traitement qu'une publication refusée.
- [ ] **Étape 4 :** client réel, selon le verdict de la tâche 1.
  - **Crate validée.** `connecter` : `client_id` parsé en `u64` (échec → genre `client_id invalide`), `Client::new`, `on_ready` vers un canal, `start()`, `recv_timeout(5 s)` (échec → `shutdown()`, genre `délai dépassé`). `publier` → `set_activity` (details, state, `timestamps.start`, assets). `effacer` → `clear_activity`. `fermer` → `shutdown`.
  - **IPC à la main.** Socket `$TMPDIR/discord-ipc-0` à `-9`. Trames : opcode `u32` LE, longueur `u32` LE, JSON. Poignée de main : op 0 `{"v":1,"client_id":…}`, réponse lue en 5 s au plus. Activité : op 1 `{"cmd":"SET_ACTIVITY","args":{"pid":<pid>,"activity":…|null},"nonce":"<compteur>"}`. Fermeture : op 2.
  - Pas de test unitaire : la sonde et le protocole manuel couvrent ce client.
- [ ] **Étape 5 :** `cargo test presence` → passe. Commit `feat(rust): présence Discord et pause`.

---

### Task 9: `journal` et `daemon`

**Fichiers :**
- Créer : `src/journal.rs`, `src/daemon.rs`, `tests/integration.rs` ; compléter `tests/commun/mod.rs` (`FauxClient` à journal partagé, `envoyer_hook(home, payload)` qui lance `sh -c COMMANDE_HOOK` avec `HOME` et `PATH` seulement, `attendre(condition, 5 s)`)

**Interfaces :**
- Consomme : tout ce qui précède.
- Produit :
  - `pub fn configurer_logs(dossier: &Path)` : logger `log`. Fichier `daemon.log`, rotation à 1 000 000 octets avec `daemon.log.1` à `.3`. Ligne au format `2026-11-20 10:00:00,123 INFO discord_rich_claude.daemon message` (niveaux `DEBUG`/`INFO`/`WARNING`/`ERROR`, `::` → `.`). `DRC_LOG` invalide → INFO et le warning `DRC_LOG invalide (<valeur>), repli sur INFO`.
  - `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Mode { Allume, Pause, Arrete }`, avec `code()` (`"allume"`/`"pause"`/`"arrete"`) et `libelle()` (`"Allumé"`/`"En pause"`/`"Arrêté"`)
  - `pub struct Daemon`, avec `new(config: ConfigSurveillee, presence: Presence, suivi: SuiviTokens, projets_claude: PathBuf, pid_vivant: Box<dyn Fn(i32) -> bool + Send + Sync>, horloge: Box<dyn Fn() -> f64 + Send + Sync>) -> Arc<Daemon>`, `recevoir(&self, ev: Evenement)`, `etat(&self) -> Value` (`{"discord", "sessions", "etat"}`), `carte(&self) -> Option<Carte>`, `controler(&self)`, `publier(&self)`, `mode(&self) -> Mode` et `changer_mode(&self, mode: Mode)`
  - `impl Gestionnaire for Daemon`
  - `pub fn demarrer(daemon: &Arc<Daemon>, chemin_socket: &Path) -> Result<EnMarche, ErreurDemarrage>` : lie le socket, lance le contrôle (30 s, premier tour immédiat) et la carte (1 s), loggue `daemon à l'écoute sur <socket>`
  - `impl EnMarche { pub fn arreter(self) }` : arrête les threads, `presence.fermer()`, supprime le socket, loggue `daemon arrêté`

- [ ] **Étape 1 :** porter les 6 tests de `tests/test_daemon.py` (dans `tests/integration.rs` ceux qui passent par le vrai `curl`), plus les modes :

```rust
#[test]
fn arreter_ignore_les_hooks_et_ferme_discord() {
    // session active, carte publiée ; changer_mode(Arrete) → journal finit par "effacer", "fermer" ;
    // registre vidé ; un hook via curl → code 0, etat()["sessions"] == 0, etat()["etat"] == "arrete"
}
#[test]
fn pause_puis_allumer_republie_la_carte_courante() {
    // changer_mode(Pause) → "effacer", pas "fermer" ; les hooks continuent de mettre à jour les sessions ;
    // changer_mode(Allume) + 15 s d'horloge → la carte courante est publiée
}
#[test]
fn allumer_apres_arreter_reconstruit_depuis_les_hooks() { /* un hook après Allume recrée la session */ }
```

- [ ] **Étape 2 :** `cargo test` → échec.
- [ ] **Étape 3 :** implémenter en suivant `daemon.py`, avec ces règles d'assemblage :
  - état partagé `Mutex<EtatDaemon>` (registre, config, `par_session`, `tokens_jour`, mode) ; `Mutex<SuiviTokens>` et `Mutex<Presence>` à part ;
  - ordre des verrous : jamais `presence` ni `suivi` pris pendant qu'on tient `EtatDaemon`. La lecture des transcripts et l'appel à Discord se font hors du verrou d'état, pour que les hooks soient toujours servis ;
  - chaque boucle exécute son action sous `catch_unwind`. Une même erreur ou panique n'est signalée qu'une fois en ERROR (`<nom> en erreur : <genre>`), puis `<nom> rétabli` en INFO ;
  - modes. `Pause` : `effacer_carte()` à la transition, puis la boucle carte ne soumet plus rien. `Arrete` : `presence.fermer()`, registre vidé, `sur_evenement` rend `Ok(())` sans rien appliquer. `Allume` : la boucle reprend `soumettre` ;
  - `client_id` absent : ERROR au premier contrôle, puis seulement après un rechargement de la config.
- [ ] **Étape 4 :** `cargo test` → tout passe, y compris `tests/integration.rs` avec le vrai `curl`.
- [ ] **Étape 5 :** commit `feat(rust): daemon, modes et journal`.

---

### Task 10: `installation` et `bundle`

**Fichiers :**
- Créer : `src/installation.rs`, `src/bundle.rs`
- Modifier : `rust/oracle/src/port.rs` (`ajouter_hooks`, `retirer_hooks`)

**Interfaces :**
- Consomme : `chemins::{LABEL, COMMANDE_HOOK}`.
- Produit :
  - `pub const MARQUEUR: &str = "discord-rich-claude"; pub const EVENEMENTS_HOOKS: [&str; 8]` (ordre de `installation.py`)
  - `pub fn ajouter_hooks(settings: &Value, commande: &str) -> Result<Value, String>` et `pub fn retirer_hooks(settings: &Value) -> Result<Value, String>`
  - `pub enum Resultat { Inchange, Refuse, Modifie }`, affiché `inchangé`/`refusé`/`modifié`
  - `pub fn modifier_settings(chemin: &Path, transformer: impl Fn(&Value) -> Result<Value, String>, confirmer: impl FnOnce(&str) -> bool, horloge: impl Fn() -> chrono::NaiveDateTime) -> Result<Resultat, String>`
  - `pub struct Sortie { pub code: i32, pub stderr: Vec<u8> }` et `pub type Executeur<'a> = &'a mut dyn FnMut(&[&str]) -> io::Result<Sortie>;`
  - `pub fn generer_plist(executable: &Path, dossier_logs: &Path) -> Vec<u8>` : XML plist, clés triées (`Label`, `ProgramArguments` = [executable, `run`], `RunAtLoad` vrai, `StandardErrorPath`, `StandardOutPath`), **sans `KeepAlive`**, texte échappé pour le XML
  - `pub fn installer_launch_agent(chemin_plist: &Path, contenu: &[u8], uid: u32, executer: Executeur, dormir: &mut dyn FnMut(Duration)) -> Result<(), ErreurLaunchctl>` et `pub fn desinstaller_launch_agent(chemin_plist: &Path, uid: u32, executer: Executeur) -> io::Result<()>`
  - `pub enum ErreurLaunchctl { Io(io::Error), Echec { code: i32, stderr: String } }`
  - `bundle` : `pub const NOM_BUNDLE: &str = "Discord Rich Claude.app";`, `pub fn info_plist(version: &str) -> Vec<u8>` (`CFBundleExecutable` = `discord-rich-claude`, `CFBundleIdentifier` = `local.discord-rich-claude`, `CFBundleName` = `Discord Rich Claude`, `CFBundlePackageType` = `APPL`, `CFBundleShortVersionString`/`CFBundleVersion` = version, `LSUIElement` vrai), `pub fn binaire_du_bundle(applications: &Path) -> PathBuf`, `pub fn installer_bundle(applications: &Path, binaire_source: &Path, executer: Executeur) -> Result<PathBuf, String>` et `pub fn desinstaller_bundle(applications: &Path) -> io::Result<()>`

- [ ] **Étape 1 :** porter les 38 tests de `tests/test_installation.py`. Adapter ceux du plist : plus de `KeepAlive`, et `assert!(!texte.contains("KeepAlive"))`. Ajouter :

```rust
#[test]
fn ajouter_hooks_garde_l_ordre_des_cles() {
    let s: Value = serde_json::from_str(r#"{"zeta":1,"model":"x","alpha":{"b":1,"a":2}}"#).unwrap();
    let n = ajouter_hooks(&s, COMMANDE_HOOK).unwrap();
    let cles: Vec<_> = n.as_object().unwrap().keys().cloned().collect();
    assert_eq!(cles, ["zeta", "model", "alpha", "hooks"]);
    assert_eq!(n["alpha"].as_object().unwrap().keys().collect::<Vec<_>>(), ["b", "a"]);
}
#[test]
fn installer_bundle_copie_signe_et_pointe_dans_le_bundle() {
    // tmp/Applications ; binaire source factice ; executer factice qui journalise
    // → Contents/MacOS/discord-rich-claude présent, exécutable (0o755) ; Info.plist contient LSUIElement
    // → executer appelé avec ["codesign", "--force", "--sign", "-", <chemin .app>]
    // → retour == binaire_du_bundle(applications)
}
#[test]
fn installer_bundle_depuis_le_bundle_lui_meme_ne_casse_rien() {
    // binaire_source == binaire_du_bundle(applications) déjà présent → contenu inchangé, pas d'erreur
}
#[test]
fn desinstaller_bundle_absent_ne_leve_pas() {}
```

- [ ] **Étape 2 :** `cargo test installation bundle` → échec.
- [ ] **Étape 3 :** implémenter en suivant `installation.py`.
  - Diff unifié avec `similar` (en-têtes `avant`/`après`) sur `serde_json::to_string_pretty`.
  - Relecture avant écriture : si le fichier a changé, `settings.json a changé pendant la confirmation : relance la commande`.
  - Sauvegarde `settings.json.bak-AAAAMMJJ-HHMMSS[-n]` en `create_new` au mode de l'original, puis `set_permissions` avant d'écrire.
  - Écriture atomique : fichier temporaire dans le même dossier, au mode de l'original, `sync_all`, puis `rename`.
  - Copie du binaire du bundle : vers un fichier temporaire, `0o755`, puis `rename`. Si la source est déjà la cible (après `canonicalize`), pas de copie.
- [ ] **Étape 4 :** `cargo test` → passe. Adaptateurs `ajouter_hooks`/`retirer_hooks`, puis diff → N/N.
- [ ] **Étape 5 :** commit `feat(rust): installation, LaunchAgent et bundle .app`.

---

### Task 11: `menu` et `cli`

**Fichiers :**
- Créer : `src/menu.rs`, `src/cli.rs`
- Modifier : `src/main.rs`

**Interfaces :**
- Consomme : `Daemon`, `Mode`, `demarrer`, `EnMarche` (tâche 9) ; installation et bundle (tâche 10) ; `interroger_etat` (tâche 7) ; `Chemins` (tâche 3).
- Produit :
  - `pub struct Entree { pub libelle: &'static str, pub active: bool }` et `pub fn entrees(mode: Mode) -> [Entree; 5]`. Dans l'ordre : ligne d'état (`mode.libelle()`, jamais active) ; « Allumer » (active si le mode n'est pas `Allume`) ; « Mettre en pause » (active seulement en `Allume`) ; « Arrêter » (active si le mode n'est pas `Arrete`) ; « Quitter » (toujours active).
  - `pub fn icone(mode: Mode) -> Vec<u8>` : RGBA 36×36, noir plus alpha, forme template. Disque plein pour `Allume`, deux barres pour `Pause`, anneau pour `Arrete`.
  - sous `#[cfg(feature = "menu")]` : `pub fn lancer_menu(daemon: Arc<Daemon>, en_marche: EnMarche) -> !`
  - `pub fn lancer(chemins: &Chemins, menu: impl FnOnce(Arc<Daemon>, EnMarche) -> i32) -> i32` (`cli`) : `run` l'appelle avec `|d, m| lancer_menu(d, m)`, les tests avec une fonction qui note l'appel
  - `pub fn principal(args: Vec<String>) -> i32` (`cli`), plus `installer_avec(chemins: &Chemins, binaire_courant: &Path, euid: u32, executer: Executeur, confirmer: impl FnOnce(&str) -> bool) -> i32`, `desinstaller_avec(...)` et `statut(chemins: &Chemins) -> i32`, injectables pour les tests

- [ ] **Étape 1 :** porter les tests de `tests/test_cli.py`. Les trois `test_executable_absolu_*` disparaissent : l'exécutable est désormais celui du bundle. `test_install_imprime_l_executable_choisi` vérifie qu'on imprime le chemin du binaire du bundle. Ajouter :

```rust
#[test]
fn entrees_selon_le_mode() {
    let e = entrees(Mode::Pause);
    assert_eq!(e[0].libelle, "En pause"); assert!(!e[0].active);
    assert!(e[1].active); assert!(!e[2].active); assert!(e[3].active); assert!(e[4].active);
}
#[test]
fn icones_distinctes_et_de_bonne_taille() {
    for m in [Mode::Allume, Mode::Pause, Mode::Arrete] { assert_eq!(icone(m).len(), 36 * 36 * 4); }
    assert_ne!(icone(Mode::Allume), icone(Mode::Pause)); assert_ne!(icone(Mode::Pause), icone(Mode::Arrete));
}
#[test]
fn status_affiche_l_etat() { /* daemon en Pause → stdout contient "état : En pause" */ }
#[test]
fn deuxieme_run_sort_en_1_avant_toute_ui() {
    // point de vigilance n° 3 : un socket déjà servi → lancer() rend 1, stderr « un daemon tourne déjà sur … »,
    // et lancer_menu n'est jamais appelé (lancer prend la fonction de menu en paramètre injectable)
}
#[test]
fn install_plist_pointe_dans_le_bundle() { /* point de vigilance n° 4 : ProgramArguments[0] == binaire_du_bundle */ }
```

- [ ] **Étape 2 :** `cargo test` → échec.
- [ ] **Étape 3 :** implémenter.
  - `run` : `configurer_logs`, construction du `Daemon` (client réel), `demarrer`. Sur `DejaLance` : message identique à `cli.py`, code 1. Sur une autre erreur d'entrée-sortie : une ligne avec le genre, code 1. Ensuite seulement, `lancer_menu`.
  - `lancer_menu` : boucle `tao` sur le fil principal, avec `ActivationPolicy::Accessory` (pas d'icône dans le Dock, même hors bundle). Icône `tray-icon` en template, tooltip `Discord Rich Claude`. Un clic de menu appelle `changer_mode`, puis l'icône et les entrées sont mises à jour. « Quitter », SIGTERM et SIGINT (`ctrlc`, via un `EventLoopProxy`) mènent à `en_marche.arreter()` puis `exit(0)`.
  - `install` : 1/4 Hooks, 2/4 Configuration, 3/4 Application (bundle), 4/4 LaunchAgent (plist vers le binaire du bundle). Messages repris de `cli.py`. `uninstall` : hooks, LaunchAgent, bundle, puis `Désinstallé. Config conservée : <config>`. Les deux refusent l'uid 0. `_confirmer` est repris tel quel : `[o/N]`, fin de fichier ou Ctrl-C = refus.
  - `status` : les lignes de `cli.py`, plus `état : <libellé>`. Commande inconnue : usage sur stderr, code 2.
- [ ] **Étape 4 :** `cargo test` → passe. `cargo clippy … -D warnings` et `cargo fmt --check` → propres.
- [ ] **Étape 5 :** commit `feat(rust): barre de menus et ligne de commande`.

---

### Task 12: gate et validation manuelle

**Fichiers :**
- Modifier : `rust/oracle/known-diffs.jsonl` (écarts 1 à 5 seulement, s'ils sont observés)

- [ ] **Étape 1 :** `rust-anything diff rust/oracle` → N/N, hors écarts déclarés. Puis `rust-anything gate rust/oracle` → passe.
- [ ] **Étape 2 :** `cargo test`, `clippy -D warnings`, `fmt --check` et `uv run pytest -q` → tout est vert.
- [ ] **Étape 3 :** `cargo build --release`. Noter la taille du binaire et la RSS au repos (`ps -o rss=` après 1 min) face au daemon Python actuel.
- [ ] **Étape 4 (Victor, manuel) :** dans l'ordre :
  1. `uv tool uninstall discord-rich-claude` ;
  2. `./rust/discord-rich-claude/target/release/discord-rich-claude install` (le « o » est à lui) ;
  3. le parcours de la spec 10 : une session, deux sessions, sous-agents, projet autorisé ou privé, Discord redémarré ;
  4. le menu : Pause (la carte disparaît), Allumer (elle revient en 15 s au plus), Arrêter, Allumer, Quitter (rien ne relance) ;
  5. redémarrer la session macOS : l'icône est là, en « Allumé » ;
  6. `discord-rich-claude status` depuis le bundle.
- [ ] **Étape 5 :** commit `chore(rust): gate rust-anything et mesures`. Les mesures vont dans le message.

---

### Task 13: retrait du Python et documentation

**Fichiers :**
- Supprimer : `src/`, `tests/`, `sonde/`, `pyproject.toml`, `uv.lock`, `rust/oracle/`
- Déplacer : `rust/discord-rich-claude/*` vers la racine du dépôt
- Modifier : `.gitignore`, `README.md`, `CLAUDE.md`, spec 14.1

- [ ] **Étape 1 :** supprimer et déplacer avec `git rm` et `git mv`. Ensuite, `cargo test`, `clippy -D warnings` et `fmt --check` à la racine → verts.
- [ ] **Étape 2 :** mettre à jour la documentation.
  - `CLAUDE.md` : commandes `cargo` ; structure en modules Rust ; la règle « Seule dépendance d'exécution : pypresence » devient la liste fermée des crates de ce plan ; la commande Python 3.13 disparaît ; la règle des tests de socket renvoie à `dossier_court()`.
  - `README.md` : installation par `cargo build --release` puis `install`, menu, désinstallation.
  - Spec 14.1 : « Python supprimé le <date> ».
- [ ] **Étape 3 :** commit `refactor: retirer l'implémentation Python, la crate Rust passe à la racine`.

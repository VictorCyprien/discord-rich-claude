# Projets dans le menu et Clauderune — plan d'implémentation

Écarts à l'exécution : voir la spec 15 (mise à jour) ; le code fait foi sur l'ordre de migration, l'index résolu au clic et l'installation du rappel.

> **Pour les agents :** sous-skill requise : superpowers:subagent-driven-development (recommandée) ou superpowers:executing-plans, tâche par tâche. Les étapes utilisent des cases à cocher (`- [ ]`).

**But :** éditer la liste blanche (ajouter, renommer par alias, changer le dossier, retirer) depuis la barre de menus, et présenter l'app sous le nom « Clauderune » avec une icône en pixel art.

**Architecture :** la config gagne une table `[projets.alias]` (lue par `config`, appliquée par `carte`). Un module `projets` lit `config.toml` sous une forme éditable, applique une opération et le réécrit en entier. `dialogues` enveloppe `osascript`. `edition` enchaîne lecture, dialogues, opération et écriture ; le menu l'appelle sur un fil à part. Le daemon prévient le menu quand la config change. Le bundle devient `Clauderune.app`, avec une icône `.icns` embarquée.

**Pile :** Rust 2024, toml 0.8, tray-icon 0.19 (muda), tao 0.31, `/usr/bin/osascript`.

**Spec :** `docs/superpowers/specs/2026-09-18-discord-rich-claude-design.md`, section 15 (elle fait foi), avec 7.1, 7.2, 14.3 et 14.4.

## Contraintes globales

- Tout en français : code, tests, libellés, commits. Libellés de la spec 15.2, au caractère près (`…` est un seul caractère U+2026).
- Aucune crate ajoutée. Liste fermée : serde, serde_json, indexmap, toml, chrono, similar, libc, log, tray-icon, tao, ctrlc ; tempfile en dev.
- Aucun accès réseau. `osascript` s'appelle par son chemin absolu `/usr/bin/osascript`.
- Les tests ne touchent jamais les vrais chemins (`~/.claude`, `~/.config`, `~/Library`, `~/Applications`, `~/.local/bin`) : `tempfile` partout, `HOME` passé en paramètre (`home: Option<&OsStr>`), jamais lu dans un test. Aucun test ne lance le vrai `osascript`, `launchctl` ni `codesign` : lanceurs injectables.
- Aucun test n'appelle `principal` ni `principal_vers` sans argument.
- Noms de tests : un test nouveau n'a pas de préfixe `test_` ; un test porté existant garde son nom.
- Les textes saisis (alias, chemins, nom prérempli) passent à `osascript` en arguments (`on run argv`), jamais collés dans une ligne `-e`.
- Journal : ni alias ni chemin dans les messages ajoutés par ce plan.
- `config.toml` écrit par le menu : 0600, atomique, jamais à travers un lien symbolique.
- Les commits se terminent par `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>` (sous-agents sonnet). Jamais `git add -A`.
- Vérification de chaque tâche : `export PATH="$HOME/.cargo/bin:$PATH"; cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --check`.

## Points de revue

1. Alias et chemins à caractères spéciaux (`"`, `\`, `'`, espace, `é`, emoji, `«»`) : un aller-retour menu → `config.toml` → daemon rend le même texte, et aucun ne peut injecter d'AppleScript. Tests : tâche 2 `aller_retour_caracteres_speciaux`, tâche 3 `aucun_texte_saisi_dans_les_lignes_e`.
2. Config modifiée à la main pendant qu'un dialogue est ouvert : l'opération s'applique à la config relue après les dialogues, pas à la copie d'avant. Test : tâche 3 `modification_concurrente_gardee`.
3. Clic sur un projet retiré ou renommé à la main depuis la construction du menu : rien ne change et aucune alerte ne s'affiche. Test : tâche 3 `projet_disparu_ne_fait_rien`.
4. Config invalide ou en lien symbolique au moment d'éditer : alerte et fichier intact à l'octet près. Tests : tâche 3 `config_invalide_alerte_sans_ecrire`, tâche 2 `ecrire_refuse_un_lien`.
5. Migration depuis `Discord Rich Claude.app` : le lien `~/.local/bin` vers l'ancien binaire est remplacé, l'ancien bundle est retiré après le LaunchAgent, jamais avant. Test : tâche 5 `install_migre_l_ancien_bundle_et_son_lien`.

---

### Tâche 1 : alias dans la config et sur la carte

**Fichiers :**
- Modifier : `src/config.rs` (struct `Config`, `valider`, nouvelles fonctions publiques)
- Modifier : `src/carte.rs` (`nom_projet`, `nom_projet_avec`, appel dans `construire_carte`, tests existants)

**Interfaces :**
- Produit :
  - `Config.alias: BTreeMap<PathBuf, String>` : clé = chemin autorisé résolu, valeur = alias.
  - `pub const ALIAS_MAX: usize = 32;`
  - `pub fn alias_valide(alias: &str) -> bool` : non vide, `alias.trim() == alias`, au plus 32 `char`, aucun `char::is_control`.
  - `pub fn resoudre_entree(p: &str, home: Option<&OsStr>) -> Result<PathBuf, String>` : tilde, refus du relatif, `resoudre(_, false)`. C'est le code actuel de la boucle de `valider`, extrait.
  - `pub(crate) fn valider(brut: &toml::Table, home: Option<&OsStr>) -> Result<Config, String>` (visibilité élargie pour la tâche 2).
  - `pub fn nom_projet(cwd: Option<&str>, autorises: &[PathBuf], alias: &BTreeMap<PathBuf, String>) -> String`.

- [ ] **Étape 1 : écrire les tests qui échouent** (`src/config.rs`, via `valider(&table, Some(home))` avec un `tempfile::TempDir` pour `home`)
  - `alias_lu_et_associe_au_chemin_resolu` : `autorises = ["<tmp>/p"]`, `[projets.alias] "<tmp>/p" = "Joli"` → `c.alias == {canonicalize(<tmp>/p): "Joli"}`.
  - `alias_cle_ecrite_en_tilde` : `autorises = ["<home>/p"]` en absolu, clé `"~/p"` → accepté, même clé résolue.
  - `alias_pour_chemin_non_autorise_refuse_tout` → `Err`.
  - `alias_hors_regles_refuse_tout` : pour chaque valeur `""`, `" X"`, `"X "`, `"a".repeat(33)`, `"a\nb"`, `"a\u{7}b"`, l'entier `42` → `Err`.
  - `alias_32_caracteres_accentues_accepte` : `"é".repeat(32)` → `Ok`.
  - `alias_table_non_table_refuse_tout` : `[projets] alias = "x"` → `Err`.
  - `config_sans_alias_a_une_table_vide` : config actuelle sans `[projets.alias]` → `c.alias.is_empty()`.
- [ ] **Étape 2 : écrire les tests de carte qui échouent** (`src/carte.rs`)
  - `nom_projet_affiche_l_alias` : cwd = le projet → `"Joli"` ; cwd = un sous-dossier du projet → `"Joli"`.
  - `alias_d_un_autre_projet_ignore` : deux projets autorisés, alias sur le second, cwd dans le premier → nom du dossier du premier.
  - Les tests `test_nom_projet_*` et `test_carte_*` existants passent `&BTreeMap::new()` en plus, sans changer de nom ni d'assertion.
- [ ] **Étape 3 : lancer les tests pour les voir échouer** : `cargo test --lib alias` → erreurs de compilation (champ `alias`, fonctions absentes).
- [ ] **Étape 4 : implémenter.**
  - `valider` lit `brut["projets"]["alias"]` (absent = table vide). Chaque clé passe par `resoudre_entree` et doit égaler un chemin résolu de `autorises`, sinon `Err("alias pour un chemin non autorisé : {clé}")`. Chaque valeur doit être un texte qui passe `alias_valide`, sinon `Err("alias invalide pour {clé}")`.
  - `nom_projet_avec` rend `alias.get(autorise)` s'il existe, sinon le nom du dossier. `construire_carte` passe `&config.alias`.
- [ ] **Étape 5 : vérifier** : la commande de vérification globale est verte.
- [ ] **Étape 6 : commit** : `git add src/config.rs src/carte.rs` puis `feat(config): alias de projets dans [projets.alias]`.

---

### Tâche 2 : module `projets`, config éditable

**Fichiers :**
- Créer : `src/projets.rs`
- Modifier : `src/lib.rs` (`pub mod projets;`)

**Interfaces :**
- Consomme : `config::{valider, resoudre_entree, alias_valide, CONFIG_PAR_DEFAUT}`, `installation::ecrire_fichier_atomique(cible, contenu, mode)`.
- Produit :
  ```rust
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub enum ErreurEdition { DejaPresent, NomInvalide, ConfigInvalide, LienSymbolique, Ecriture, ProjetAbsent }
  impl ErreurEdition { pub fn message(self) -> Option<&'static str> }   // ProjetAbsent → None
  #[derive(Clone, Debug, PartialEq, Eq)]
  pub struct ProjetAffiche { pub nom: String, pub chemin: String }       // chemin tel qu'écrit
  pub struct ConfigEditable { /* privé */ }
  impl ConfigEditable {
      pub fn lire(chemin: &Path, home: Option<&OsStr>) -> Result<Self, ErreurEdition>;
      pub fn affiches(&self) -> Vec<ProjetAffiche>;
      pub fn nom_affiche(&self, chemin: &str) -> Option<String>;
      pub fn contient(&self, dossier: &Path, home: Option<&OsStr>) -> bool;
      pub fn ajouter(&mut self, dossier: &Path, saisi: &str, home: Option<&OsStr>) -> Result<(), ErreurEdition>;
      pub fn renommer(&mut self, chemin: &str, saisi: &str) -> Result<(), ErreurEdition>;
      pub fn changer_dossier(&mut self, chemin: &str, nouveau: &Path, home: Option<&OsStr>) -> Result<(), ErreurEdition>;
      pub fn retirer(&mut self, chemin: &str) -> Result<(), ErreurEdition>;
      pub fn rendre(&self) -> String;
      pub fn ecrire(&self, cible: &Path) -> Result<(), ErreurEdition>;
  }
  pub fn nom_dossier(dossier: &Path) -> String;                          // dernier élément du chemin résolu
  pub fn forme_ecrite(dossier: &Path, home: Option<&OsStr>) -> String;
  ```
- Messages de `ErreurEdition::message`, exacts (spec 15.2) :
  - `DejaPresent` : `Ce dossier est déjà dans la liste.`
  - `NomInvalide` : `Nom invalide : 1 à 32 caractères, sans caractère de contrôle.`
  - `ConfigInvalide` : `config.toml est invalide : corrige-la avant de modifier les projets.`
  - `LienSymbolique` : `config.toml est un lien symbolique : modifie-la à la main.`
  - `Ecriture` : `Impossible d'écrire config.toml (voir le journal).`

Règles fixées :
- `lire` : `symlink_metadata` est un lien → `LienSymbolique`. Fichier absent → config vide aux valeurs par défaut. Illisible, TOML invalide ou `valider` en échec → `ConfigInvalide`. Chaque entrée garde sa chaîne telle qu'écrite, son chemin résolu (`resoudre_entree`) et son alias (clé d'alias rattachée par chemin résolu). `client_id`, `inactif_apres_min`, `masquer_apres_min` et `rotation_s` gardent leur `toml::Value` d'origine.
- Saisie d'un nom (`ajouter`, `renommer`) : `saisi.trim()`. Vide ou égal à `nom_dossier` → pas d'alias. Sinon `alias_valide` doit passer, sinon `NomInvalide` sans rien changer.
- `ajouter` et `changer_dossier` : un dossier dont le chemin résolu est déjà dans la liste → `DejaPresent`. `changer_dossier` garde l'alias. Un `chemin` inconnu de la liste → `ProjetAbsent` (`renommer`, `changer_dossier`, `retirer`). `retirer` retire aussi l'alias.
- `forme_ecrite` : retire un `/` final (sauf pour `/`). Sous `home` : `~/reste`, et `home` lui-même donne `~`. Sinon le chemin absolu tel quel.
- `affiches` : tri par `nom.to_lowercase()`.
- `ecrire` : cible en lien symbolique → `LienSymbolique`. Crée le dossier parent. `ecrire_fichier_atomique(cible, rendre(), 0o600)`. En cas d'échec : `log::warn!("écriture de config.toml impossible : {:?}", e.kind())` puis `Ecriture`.
- `rendre` : texte exact ci-dessous. Chaînes, clés d'alias et nombres rendus par `toml::Value::to_string()`. Valeur absente : `client_id = ""`, `10`, `60`, `20`. `autorises = []` si la liste est vide. Table `[projets.alias]` omise sans alias, et ses lignes suivent l'ordre de `autorises`. Un nouveau projet s'ajoute en fin de liste.

```toml
# Configuration de discord-rich-claude.
# ID de l'application Discord : Developer Portal > General Information > Application ID.
client_id = "123"
inactif_apres_min = 10
masquer_apres_min = 60
rotation_s = 20

[projets]
# Refusé par défaut : seuls ces dossiers (chemins complets) affichent leur nom.
autorises = [
  "~/W/AURA",
  "~/W/discord-rich-claude",
]

[projets.alias]
# Nom affiché sur Discord à la place du nom du dossier.
"~/W/discord-rich-claude" = "DiscordRichClaude"
```

- [ ] **Étape 1 : écrire les tests qui échouent** (`src/projets.rs`, `home` = un `TempDir`, dossiers créés dedans)
  - `rendre_format_exact` : le texte ci-dessus, à l'octet près.
  - `rendre_sans_alias_omet_la_table` ; `rendre_liste_vide` (`autorises = []`).
  - `lire_absent_valeurs_par_defaut` : `rendre()` contient `client_id = ""`, `inactif_apres_min = 10`, `masquer_apres_min = 60`, `rotation_s = 20`, `autorises = []`.
  - `lire_garde_la_forme_ecrite` : `"~/p"` reste `"~/p"`, `inactif_apres_min = 10.5` reste `10.5`, `client_id = 123` reste un entier.
  - `lire_config_invalide` → `ConfigInvalide` ; `lire_lien_symbolique` → `LienSymbolique`.
  - `forme_ecrite_tilde_sous_home` : `<home>/W/X/` → `~/W/X` ; `<home>` → `~` ; `/opt/X` → `/opt/X`.
  - `ajouter_deja_present_compare_les_chemins_resolus` : un lien symbolique vers un dossier déjà autorisé → `DejaPresent`.
  - `ajouter_nom_vide_ou_egal_au_dossier_sans_alias`.
  - `renommer_pose_puis_retire_l_alias` ; `renommer_33_caracteres_refuse_sans_changer` (`rendre()` inchangé).
  - `changer_dossier_garde_l_alias` ; `changer_dossier_vers_un_dossier_deja_present` → `DejaPresent`.
  - `retirer_retire_aussi_l_alias`.
  - `chemin_inconnu_projet_absent` pour `renommer`, `changer_dossier` et `retirer`.
  - `affiches_tries_sans_casse_et_alias_prioritaire` : dossiers `b`, `A`, `c` avec alias `Zed` sur `c` → noms `A`, `b`, `Zed`.
  - `aller_retour_caracteres_speciaux` : dossier `<home>/Mon "dossier" l'été`, alias `Mon "Projet" \ é🦀 «x»`. `ecrire` puis `ConfigEditable::lire`, et `config::valider` sur le texte rendu → même alias, même chemin résolu.
  - `ecrire_cree_le_parent_en_0600` : cible `<tmp>/a/b/config.toml`, mode final `0o600`.
  - `ecrire_resserre_un_fichier_plus_large` : original en `0o644` → `0o600`.
  - `ecrire_refuse_un_lien` : la cible est un lien → `LienSymbolique`, la cible du lien est intacte.
- [ ] **Étape 2 : lancer les tests pour les voir échouer** : `cargo test --lib projets` → le module n'existe pas.
- [ ] **Étape 3 : implémenter `src/projets.rs`** selon les règles ci-dessus.
- [ ] **Étape 4 : vérifier** : la commande de vérification globale est verte.
- [ ] **Étape 5 : commit** : `git add src/projets.rs src/lib.rs` puis `feat(projets): config éditable et réécriture de config.toml`.

---

### Tâche 3 : dialogues `osascript` et enchaînement d'une édition

**Fichiers :**
- Créer : `src/dialogues.rs`, `src/edition.rs`
- Modifier : `src/lib.rs` (`pub mod dialogues; pub mod edition;`)

**Interfaces :**
- Consomme : `projets::{ConfigEditable, ErreurEdition, nom_dossier}` (tâche 2).
- Produit, dans `dialogues` :
  ```rust
  pub const OSASCRIPT: &str = "/usr/bin/osascript";
  pub const TITRE: &str = "Clauderune";
  pub const INVITE_AJOUT: &str = "Choisir le dossier du projet";
  pub const INVITE_CHANGEMENT: &str = "Choisir le nouveau dossier du projet";
  pub const QUESTION_NOM: &str = "Nom affiché sur Discord :";
  pub struct Reponse { pub code: i32, pub stdout: String }
  pub type Lanceur<'a> = &'a mut dyn FnMut(&[&str]) -> io::Result<Reponse>;
  pub fn lanceur_reel(commande: &[&str]) -> io::Result<Reponse>;
  pub fn choisir_dossier(lancer: Lanceur, invite: &str) -> Option<PathBuf>;
  pub fn demander_nom(lancer: Lanceur, defaut: &str) -> Option<String>;
  pub fn confirmer_retrait(lancer: Lanceur, nom: &str) -> bool;
  pub fn alerter(lancer: Lanceur, message: &str);
  ```
- Produit, dans `edition` :
  ```rust
  #[derive(Clone, Debug, PartialEq, Eq)]
  pub enum Action { Ajouter, Renommer(String), ChangerDossier(String), Retirer(String) } // chemin tel qu'écrit
  pub fn editer(action: &Action, config: &Path, home: Option<&OsStr>, lancer: Lanceur) -> bool; // true = config écrite
  ```

Scripts, une ligne AppleScript par `-e` (`dialogues`). Commande : `[OSASCRIPT, "-e", l1, "-e", l2, …, arg1, arg2]`.
- Dossier : `on run argv` / `activate` / `POSIX path of (choose folder with prompt (item 1 of argv))` / `end run`. Argument : l'invite.
- Nom : `on run argv` / `activate` / `text returned of (display dialog (item 1 of argv) default answer (item 2 of argv) with title "Clauderune" buttons {"Annuler", "OK"} default button "OK" cancel button "Annuler")` / `end run`. Arguments : `QUESTION_NOM`, `defaut`.
- Retrait : `on run argv` / `activate` / `display dialog (item 1 of argv) with title "Clauderune" buttons {"Annuler", "Retirer"} default button "Annuler" cancel button "Annuler"` / `end run`. Argument : `format!("Retirer « {nom} » des projets affichés ?")`.
- Alerte : `on run argv` / `activate` / `display alert "Clauderune" message (item 1 of argv) as warning buttons {"OK"} default button "OK"` / `end run`. Argument : le message.
- Le premier argument après les scripts est toujours un texte fixe qui ne commence pas par `-` : `getopt` d'`osascript` s'arrête au premier non-option, la saisie ne peut donc pas devenir une option.
- Lecture : code ≠ 0 ou erreur d'E/S → annulé (`None` ou `false`). `stdout` perd exactement un `\n` final. Un dossier vide → `None`.

`editer`, dans l'ordre :
1. `ConfigEditable::lire`. Erreur → `alerter(message)` si `message()` est `Some`, puis `false`.
2. Dialogues, avec les valeurs de cette première lecture :
   - `Ajouter` : `choisir_dossier(INVITE_AJOUT)`. Si `contient` → alerte `DejaPresent`, fin. Sinon `demander_nom(nom_dossier(dossier))`.
   - `Renommer(c)` : `nom_affiche(c)`, `None` → `false` sans dialogue. Puis `demander_nom(nom affiché)`.
   - `ChangerDossier(c)` : projet absent → `false` sans dialogue. Puis `choisir_dossier(INVITE_CHANGEMENT)`.
   - `Retirer(c)` : `nom_affiche(c)`, absent → `false`. Puis `confirmer_retrait(nom)`.
   - Un dialogue annulé → `false`.
3. Relire la config (`ConfigEditable::lire`), appliquer l'opération sur cette copie fraîche, puis `ecrire`. Toute erreur → alerte si `message()` est `Some`, puis `false`.
4. Succès : `log::info!("projets modifiés depuis le menu")`, puis `true`.

- [ ] **Étape 1 : écrire les tests de `dialogues` qui échouent** (faux lanceur qui enregistre la commande et rend une `Reponse` fixée)
  - `commande_commence_par_osascript_absolu` : `commande[0] == "/usr/bin/osascript"`.
  - `aucun_texte_saisi_dans_les_lignes_e` : `demander_nom` avec `defaut = "x\" & (do shell script \"id\") & \""` et `confirmer_retrait` avec le même texte. Aucune ligne qui suit un `-e` ne contient `do shell script`. Le texte apparaît tel quel comme dernier argument.
  - `premier_argument_fixe` : pour les 4 fonctions, le premier argument après les scripts vaut l'invite, `QUESTION_NOM`, un texte qui commence par `Retirer « ` ou le message d'alerte.
  - `demander_nom_retire_un_seul_saut_de_ligne` : stdout `"  Joli \n"` → `Some("  Joli ")` (le `trim` se fait dans `projets`).
  - `annulation_code_1` : `choisir_dossier` → `None`, `demander_nom` → `None`, `confirmer_retrait` → `false`.
  - `choisir_dossier_rend_le_chemin` : stdout `"/tmp/a b/\n"` → `Some(PathBuf::from("/tmp/a b/"))`.
- [ ] **Étape 2 : écrire les tests d'`edition` qui échouent** (`src/edition.rs`). Le faux lanceur dépile des `Reponse` scriptées et enregistre chaque commande. `home` et la config sont dans un `TempDir`.
  - `ajouter_complet_ecrit_la_config` : réponses dossier `"<home>/P/\n"`, nom `"Joli\n"` → `true`, la config relue contient `~/P` avec l'alias `Joli`.
  - `config_absente_ajout_cree_le_fichier` : config dans `<tmp>/x/y/config.toml` absente → créée en `0o600`.
  - `annuler_le_dossier_ne_change_rien` : code 1 → `false`, fichier identique à l'octet près, un seul appel.
  - `ajouter_deja_present_alerte_sans_demander_le_nom` : 2 appels exactement (dossier, alerte). Le dernier argument de l'alerte est `Ce dossier est déjà dans la liste.`.
  - `config_invalide_alerte_sans_ecrire` : TOML invalide → 1 appel (alerte avec le message `ConfigInvalide`), fichier identique à l'octet près.
  - `renommer_nom_invalide_alerte` : 33 caractères → alerte `NomInvalide`, fichier identique.
  - `retirer_confirme` et `retirer_annule`.
  - `projet_disparu_ne_fait_rien` : `Renommer("~/absent")` → `false`, aucun appel au lanceur.
  - `modification_concurrente_gardee` : le faux lanceur réécrit la config pendant le dialogue du nom, pour ajouter un autre projet. Le résultat contient les deux projets.
- [ ] **Étape 3 : lancer les tests pour les voir échouer** : `cargo test --lib -- dialogues edition` → modules absents.
- [ ] **Étape 4 : implémenter** `src/dialogues.rs` puis `src/edition.rs`. `lanceur_reel` : `std::process::Command::new(commande[0]).args(&commande[1..]).output()`, puis `code` (ou `-1`) et `stdout` en `from_utf8_lossy`.
- [ ] **Étape 5 : vérifier** : la commande de vérification globale est verte.
- [ ] **Étape 6 : commit** : `git add src/dialogues.rs src/edition.rs src/lib.rs` puis `feat(edition): dialogues osascript pour éditer les projets`.

---

### Tâche 4 : menu « Projets » et rechargement

**Fichiers :**
- Modifier : `src/daemon.rs` (champ de rappel, trois méthodes, `controler`)
- Modifier : `src/menu.rs` (aides pures, sous-menu, clics, `Evt::ProjetsChanges`)

**Interfaces :**
- Consomme : `edition::{editer, Action}`, `dialogues::lanceur_reel`, `projets::{ConfigEditable, ErreurEdition, ProjetAffiche}`.
- Produit, dans `daemon` :
  ```rust
  pub fn chemin_config(&self) -> PathBuf;
  pub fn recharger_config(&self) -> bool;                                  // recharge si le fichier a changé, puis rappel
  pub fn sur_config_changee(&self, rappel: Box<dyn Fn() + Send + Sync>);
  ```
  Le rappel est gardé dans `Mutex<Option<Box<dyn Fn() + Send + Sync>>>`. Il est appelé chaque fois que `recharger_si_change()` rend `true` (dans `controler`, mode Arrêté compris, et dans `recharger_config`), toujours **après** avoir relâché le verrou d'état, jamais sous un autre verrou du daemon.
- Produit, dans `menu` (hors `cfg(feature = "menu")`, testable) :
  ```rust
  pub const ID_AJOUTER: &str = "projets:ajouter";
  #[derive(Clone, Copy, Debug, PartialEq, Eq)]
  pub enum ActionMenu { Renommer, ChangerDossier, Retirer }
  pub fn id_projet(action: ActionMenu, index: usize) -> String;          // "projets:renommer:3", "projets:changer:3", "projets:retirer:3"
  pub fn lire_id_projet(id: &str) -> Option<(ActionMenu, usize)>;
  #[derive(Debug, PartialEq, Eq)]
  pub enum LignesProjets { Liste(Vec<ProjetAffiche>), Invalide }
  pub fn lignes_projets(config: &Path, home: Option<&OsStr>) -> LignesProjets; // ConfigInvalide ou LienSymbolique → Invalide
  ```
- Libellés : `Projets`, `Aucun projet`, `config.toml invalide`, `Ajouter un projet…`, `Renommer…`, `Changer le dossier…`, `Retirer…`.

Construction (`cfg(feature = "menu")`, ordre de la spec 15.2) : la ligne d'état, les trois actions, un séparateur, le sous-menu `Projets`, un séparateur, `Quitter`. Le sous-menu contient :
- pour chaque projet, un sous-menu titré par son nom, avec le chemin grisé, un séparateur, puis les trois actions ;
- sinon la ligne grisée `Aucun projet` ou `config.toml invalide` ;
- puis un séparateur et `Ajouter un projet…`.

`Interface` garde le `Submenu` « Projets » et un `Vec<String>` (index → chemin tel qu'écrit). Reconstruction : on vide le sous-menu (`remove` de chaque item) et on le remplit de nouveau.

- Clic sur `ID_AJOUTER` ou sur un id lu par `lire_id_projet` : on construit l'`Action` à partir du chemin de l'index, puis un fil à part. Une `static EDITION_EN_COURS: AtomicBool` passe de `false` à `true` par `compare_exchange` ; en cas d'échec, le clic est ignoré. Le fil :
  - lance `editer(&action, &daemon.chemin_config(), std::env::var_os("HOME").as_deref(), &mut lanceur)` ;
  - si le résultat est `true`, appelle `daemon.recharger_config()` ;
  - remet le drapeau à `false`.
- À `StartCause::Init`, après `creer_interface` : `daemon.sur_config_changee(...)`. Le rappel envoie `Evt::ProjetsChanges` par un proxy gardé dans un `Mutex`, comme `proxy_menu`. `Evt::ProjetsChanges` déclenche la reconstruction du sous-menu.

- [ ] **Étape 1 : écrire les tests qui échouent**
  - `daemon` : `rappel_appele_quand_la_config_change_au_controle`. Un compteur `Arc<AtomicUsize>` ; on modifie le fichier, puis `controler()` → 1 ; un second `controler()` sans modification → toujours 1.
  - `daemon` : `rappel_appele_aussi_en_mode_arrete` (après `changer_mode(Mode::Arrete)` ; voir les tests existants du daemon pour un `Presence` factice).
  - `daemon` : `recharger_config_sans_changement_rend_faux`.
  - `menu` : `id_projet_aller_retour` pour les 3 actions et l'index 0 et 41. `lire_id_projet` rend `None` pour `"projets:ajouter"`, `"projets:renommer:"`, `"projets:renommer:x"`, `"allumer"`.
  - `menu` : `lignes_projets_triees` et `lignes_projets_config_invalide` (fichier TOML invalide → `Invalide`).
- [ ] **Étape 2 : lancer les tests pour les voir échouer** : `cargo test --lib -- rappel id_projet lignes_projets` → erreurs de compilation.
- [ ] **Étape 3 : implémenter** `daemon` puis `menu`.
- [ ] **Étape 4 : vérifier** : la commande de vérification globale est verte, plus `cargo build --release` sans avertissement.
- [ ] **Étape 5 : commit** : `git add src/daemon.rs src/menu.rs` puis `feat(menu): sous-menu Projets et rechargement de la config`.

---

### Tâche 5 : Clauderune (bundle, migration, icônes, docs)

**Fichiers :**
- Modifier : `src/bundle.rs`, `src/cli.rs`, `src/menu.rs` (tooltip, `icone`), `tests/cli.rs` (ligne 640 et nouveaux tests), `README.md`, `CLAUDE.md`
- Existants, à utiliser tels quels : `assets/AppIcon.icns`, `assets/clauderune.png`

**Interfaces :**
- Produit, dans `bundle` :
  ```rust
  pub const NOM_BUNDLE: &str = "Clauderune.app";
  pub const NOM_ANCIEN_BUNDLE: &str = "Discord Rich Claude.app";
  pub const ICONE_APP: &[u8] = include_bytes!("../assets/AppIcon.icns");
  pub fn binaire_de_l_ancien_bundle(applications: &Path) -> PathBuf;
  pub enum EtatLien { Absent, Correct, Ancien, Occupe }                   // Ancien = lien vers le binaire de l'ancien bundle
  pub fn etat_lien(lien: &Path, applications: &Path) -> io::Result<EtatLien>;
  pub fn poser_lien(lien: &Path, applications: &Path) -> io::Result<()>;  // Absent : crée ; Ancien : remplace ; Correct : rien ; Occupe : Err(AlreadyExists)
  pub fn retirer_lien(lien: &Path, applications: &Path) -> io::Result<bool>; // retire Correct ou Ancien
  pub fn retirer_ancien_bundle(applications: &Path) -> io::Result<bool>;  // true si retiré ; un lien → Err, jamais suivi
  pub fn desinstaller_bundle(applications: &Path) -> io::Result<()>;      // retire les deux bundles
  ```
- `info_plist(version)` : clés triées, `CFBundleDisplayName` = `Clauderune`, `CFBundleExecutable`, `CFBundleIconFile` = `AppIcon`, `CFBundleIdentifier` = `local.discord-rich-claude` (inchangé), `CFBundleName` = `Clauderune`, `CFBundlePackageType`, `CFBundleShortVersionString`, `CFBundleVersion`, `LSUIElement`.
- `installer_bundle` écrit `Contents/Resources/AppIcon.icns` (`ICONE_APP`, 0o644, atomique) **avant** `codesign`.
- `poser_lien` sur `Ancien` : lien temporaire frère, puis `rename` par-dessus, pour ne jamais laisser de trou.
- `cli` :
  - `install` refuse toujours sur `Occupe` (message inchangé).
  - Après l'étape 4 réussie, `retirer_ancien_bundle`. Si `true`, sortie `  ancienne app retirée : <chemin>`. En cas d'erreur, avertissement sur la sortie d'erreur `  ancienne app laissée en place : <chemin>`, et l'installation continue.
  - Étape 5 : sur `Ancien`, sortie `  lien mis à jour : <lien> -> <binaire>`.
  - `uninstall` retire un lien `Correct` ou `Ancien`.
- `menu` : tooltip `Clauderune`. `icone(mode)` rend 36 × 36 RGBA, noir et alpha, depuis trois constantes `GRILLE_ALLUME`, `GRILLE_PAUSE` et `GRILLE_ARRETE` de 18 lignes de 18 caractères. Chaque `#` couvre 2 × 2 pixels opaques, chaque `.` 2 × 2 pixels transparents. `couverture_disque` disparaît. Grilles exactes :

```text
GRILLE_ALLUME            GRILLE_PAUSE             GRILLE_ARRETE
..................       ..................       ..................
...#############..       ...#############..       ...#############..
..###############.       ..#.............#.       ..#.............#.
..#######.#######.       ..#.............#.       ..#.............#.
..####.##.##.####.       ..#...##...##...#.       ..#.............#.
..#####.#.#.#####.       ..#...##...##...#.       ..#.............#.
..######...######.       ..#...##...##...#.       ..#.............#.
..###.........###.       ..#...##...##...#.       ..#.............#.
..######...######.       ..#...##...##...#.       ..#.............#.
..#####.#.#.#####.       ..#...##...##...#.       ..#.............#.
..####.##.##.####.       ..#...##...##...#.       ..#.............#.
..#######.#######.       ..#.............#.       ..#.............#.
..###############.       ..#.............#.       ..#.............#.
...#############..       ...#...#########..       ...#...#########..
....###...........       ....#.#...........       ....#.#...........
....##............       ....##............       ....##............
....#.............       ....#.............       ....#.............
..................       ..................       ..................
```

- [ ] **Étape 1 : écrire les tests qui échouent**
  - `bundle` : `info_plist_au_format_plistlib` mis à jour avec la liste de clés ci-dessus (texte exact).
  - `bundle` : `installer_bundle_ecrit_l_icone` : `Resources/AppIcon.icns` égal à `ICONE_APP`, qui commence par `b"icns"`.
  - `bundle` : `etat_lien_ancien` ; `poser_lien_remplace_l_ancien` (le lien vise ensuite le nouveau binaire) ; `poser_lien_refuse_occupe` (fichier ordinaire intact) ; `retirer_lien_retire_l_ancien`.
  - `bundle` : `retirer_ancien_bundle_absent_rend_faux`, `retirer_ancien_bundle_present_rend_vrai`, `retirer_ancien_bundle_refuse_un_lien` (la cible du lien est intacte).
  - `bundle` : `desinstaller_bundle_retire_les_deux`.
  - `tests/cli.rs` : `install_migre_l_ancien_bundle_et_son_lien`. Avant l'appel : ancien bundle présent, lien vers son binaire. Après : nouveau bundle présent, ancien absent, lien vers le nouveau binaire. Dans la sortie, la ligne `ancienne app retirée` vient après `LaunchAgent chargé`.
  - `tests/cli.rs` : `uninstall_retire_un_lien_vers_l_ancien_bundle`.
  - `menu` : `grilles_de_18_sur_18` ; `icone_suit_la_grille` (pour chaque mode, le pixel `(2x, 2y)` a un alpha de 255 si la grille a `#` en `(x, y)`, sinon 0). Les tests `icones_*` existants restent verts.
- [ ] **Étape 2 : lancer les tests pour les voir échouer** : `cargo test` → échecs attendus sur ces tests.
- [ ] **Étape 3 : implémenter** `bundle`, puis `cli`, puis `menu`.
- [ ] **Étape 4 : docs.**
  - `README.md` : le nom Clauderune (l'app `~/Applications/Clauderune.app`, la commande reste `discord-rich-claude`), une section « Projets » sur le menu, les alias et le format `[projets.alias]`, une note de migration (`install` retire l'ancienne app).
  - `CLAUDE.md` : la liste de structure gagne `projets` (config éditable, réécriture de `config.toml`), `dialogues` (`osascript`), `edition` (enchaînement d'une édition) et `assets/` (icônes). Règle ajoutée : aucun test ne lance le vrai `osascript`, le `Lanceur` est injectable.
- [ ] **Étape 5 : vérifier** : la commande de vérification globale est verte, `cargo build --release` passe, et `ls -l target/release/discord-rich-claude` donne une taille sous 1,5 Mo.
- [ ] **Étape 6 : commit** : `git add src/bundle.rs src/cli.rs src/menu.rs tests/cli.rs README.md CLAUDE.md assets/AppIcon.icns assets/clauderune.png` puis `feat: Clauderune, icônes pixel art et migration du bundle`.

---

## Après les tâches

1. Revue finale de la branche par deux relecteurs opus en parallèle :
   - **sécurité** : injection AppleScript, écriture de config, liens symboliques, migration ;
   - **cohérence** : spec 15 face au code, libellés, docs.
2. Validation manuelle par Victor, sur sa machine :
   - quitter le daemon par le menu ;
   - `cargo build --release` puis `./target/release/discord-rich-claude install`, avec son « o » ;
   - les dialogues passent-ils au premier plan (`activate`) ?
   - ajouter, renommer, changer le dossier et retirer un projet ;
   - l'alias apparaît-il sur Discord ?
   - l'icône est-elle visible dans Spotlight ?

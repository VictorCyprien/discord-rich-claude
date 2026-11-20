//! État des sessions Claude Code, en mémoire. Logique pure, sans entrée-sortie
//! (hormis `pid_vivant`). Voir la spec, section 4.

use indexmap::IndexMap;

pub const REFLECHIT: &str = "Réfléchit";
pub const ATTEND_REPONSE: &str = "Attend ta réponse";
pub const ATTEND_VALIDATION: &str = "Attend ta validation";
pub const INACTIF: &str = "Inactif";
pub const TRAVAILLE: &str = "Travaille";

pub fn libelle_outil(tool_name: Option<&str>) -> &'static str {
    match tool_name.unwrap_or("") {
        "Edit" | "Write" | "NotebookEdit" => "Écrit du code",
        "Bash" => "Lance une commande",
        "Read" | "Grep" | "Glob" => "Lit le code",
        "WebSearch" | "WebFetch" => "Cherche sur le web",
        "Agent" => "Pilote des sous-agents",
        _ => TRAVAILLE,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Evenement {
    pub nom: String,
    pub session_id: String,
    pub recu_a: f64,
    pub cwd: Option<String>,
    pub tool_name: Option<String>,
    pub agent_id: Option<String>,
    pub transcript_path: Option<String>,
    pub pid: Option<i32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    pub session_id: String,
    pub debut: f64,
    pub dernier_evenement: f64,
    pub activite: String,
    pub cwd: Option<String>,
    pub transcript_path: Option<String>,
    pub pid: Option<i32>,
    pub sous_agents: IndexMap<String, f64>,
}

pub fn activite_affichee(s: &Session, maintenant: f64, inactif_apres_s: f64) -> String {
    if maintenant - s.dernier_evenement >= inactif_apres_s {
        return INACTIF.to_string();
    }
    match s.sous_agents.len() {
        0 => s.activite.clone(),
        1 => "Pilote 1 sous-agent".to_string(),
        n => format!("Pilote {n} sous-agents"),
    }
}

pub fn pid_vivant(pid: i32) -> bool {
    // SAFETY : `kill` avec le signal 0 ne signale rien, il teste seulement l'existence.
    if unsafe { libc::kill(pid, 0) } == 0 {
        return true;
    }
    // ESRCH : aucun processus. EPERM : il existe mais appartient à un autre compte.
    std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

#[derive(Default)]
pub struct Registre {
    pub sessions: IndexMap<String, Session>,
}

impl Registre {
    pub fn appliquer(&mut self, ev: &Evenement) {
        if ev.nom == "SessionEnd" {
            self.sessions.shift_remove(&ev.session_id);
            return;
        }
        let s = self
            .sessions
            .entry(ev.session_id.clone())
            .or_insert_with(|| Session {
                session_id: ev.session_id.clone(),
                debut: ev.recu_a,
                dernier_evenement: ev.recu_a,
                activite: TRAVAILLE.to_string(),
                cwd: None,
                transcript_path: None,
                pid: None,
                sous_agents: IndexMap::new(),
            });
        s.dernier_evenement = ev.recu_a;
        if ev.pid.is_some() {
            s.pid = ev.pid;
        }

        if let Some(agent_id) = &ev.agent_id {
            // Un sous-agent compte comme activité mais ne change pas le fil principal.
            if ev.nom == "SubagentStop" {
                s.sous_agents.shift_remove(agent_id);
            } else {
                s.sous_agents.insert(agent_id.clone(), ev.recu_a);
            }
            return;
        }

        if let Some(cwd) = ev.cwd.as_ref().filter(|c| !c.is_empty()) {
            s.cwd = Some(cwd.clone());
        }
        if let Some(t) = ev.transcript_path.as_ref().filter(|t| !t.is_empty()) {
            s.transcript_path = Some(t.clone());
        }
        match ev.nom.as_str() {
            "SessionStart" | "Stop" => s.activite = ATTEND_REPONSE.to_string(),
            "UserPromptSubmit" => s.activite = REFLECHIT.to_string(),
            "PreToolUse" => s.activite = libelle_outil(ev.tool_name.as_deref()).to_string(),
            "PermissionRequest" => s.activite = ATTEND_VALIDATION.to_string(),
            _ => {}
        }
    }

    pub fn purger(
        &mut self,
        maintenant: f64,
        inactif_apres_s: f64,
        pid_vivant: impl Fn(i32) -> bool,
    ) {
        // `retain` garde l'ordre d'origine des survivants.
        self.sessions
            .retain(|_, s| !matches!(s.pid, Some(pid) if !pid_vivant(pid)));
        for s in self.sessions.values_mut() {
            s.sous_agents
                .retain(|_, t| maintenant - *t < inactif_apres_s);
        }
    }

    /// La session au dernier événement le plus récent. À égalité, la première
    /// dans l'ordre d'insertion (comme `max` en Python).
    pub fn session_affichee(&self) -> Option<&Session> {
        let mut meilleure: Option<&Session> = None;
        for s in self.sessions.values() {
            if meilleure.is_none_or(|m| s.dernier_evenement > m.dernier_evenement) {
                meilleure = Some(s);
            }
        }
        meilleure
    }

    pub fn debut_plus_ancien(&self) -> Option<f64> {
        self.sessions.values().map(|s| s.debut).reduce(f64::min)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INACTIF_S: f64 = 600.0;

    fn ev(nom: &str, t: f64) -> Evenement {
        evs(nom, t, "s1")
    }

    fn evs(nom: &str, t: f64, sid: &str) -> Evenement {
        Evenement {
            nom: nom.into(),
            session_id: sid.into(),
            recu_a: t,
            cwd: None,
            tool_name: None,
            agent_id: None,
            transcript_path: None,
            pid: None,
        }
    }

    fn outil(nom: &str, t: f64, o: Option<&str>) -> Evenement {
        Evenement {
            tool_name: o.map(String::from),
            ..ev(nom, t)
        }
    }

    fn agent(nom: &str, t: f64, id: &str) -> Evenement {
        Evenement {
            agent_id: Some(id.into()),
            ..ev(nom, t)
        }
    }

    fn registre_demarre() -> Registre {
        let mut r = Registre::default();
        r.appliquer(&ev("SessionStart", 0.0));
        r
    }

    #[test]
    fn test_session_start_cree_une_session_qui_attend() {
        let mut r = Registre::default();
        r.appliquer(&Evenement {
            cwd: Some("/p".into()),
            transcript_path: Some("/t.jsonl".into()),
            pid: Some(42),
            ..ev("SessionStart", 100.0)
        });
        let s = &r.sessions["s1"];
        assert_eq!(s.debut, 100.0);
        assert_eq!(s.cwd.as_deref(), Some("/p"));
        assert_eq!(s.transcript_path.as_deref(), Some("/t.jsonl"));
        assert_eq!(s.pid, Some(42));
        assert_eq!(activite_affichee(s, 100.0, INACTIF_S), ATTEND_REPONSE);
    }

    #[test]
    fn test_evenements_du_fil_principal() {
        let cas: Vec<(&str, Option<&str>, &str)> = vec![
            ("UserPromptSubmit", None, REFLECHIT),
            ("PreToolUse", Some("Edit"), "Écrit du code"),
            ("PreToolUse", Some("Write"), "Écrit du code"),
            ("PreToolUse", Some("NotebookEdit"), "Écrit du code"),
            ("PreToolUse", Some("Bash"), "Lance une commande"),
            ("PreToolUse", Some("Read"), "Lit le code"),
            ("PreToolUse", Some("Grep"), "Lit le code"),
            ("PreToolUse", Some("Glob"), "Lit le code"),
            ("PreToolUse", Some("WebSearch"), "Cherche sur le web"),
            ("PreToolUse", Some("WebFetch"), "Cherche sur le web"),
            ("PreToolUse", Some("Agent"), "Pilote des sous-agents"),
            ("PreToolUse", Some("mcp__plane__list_issues"), TRAVAILLE),
            ("PreToolUse", None, TRAVAILLE),
            ("PermissionRequest", Some("Bash"), ATTEND_VALIDATION),
            ("Stop", None, ATTEND_REPONSE),
        ];
        for (nom, o, attendu) in cas {
            let mut r = registre_demarre();
            r.appliquer(&outil(nom, 1.0, o));
            assert_eq!(
                activite_affichee(&r.sessions["s1"], 1.0, INACTIF_S),
                attendu,
                "{nom} {o:?}"
            );
        }
    }

    #[test]
    fn test_session_end_supprime_la_session() {
        let mut r = registre_demarre();
        r.appliquer(&ev("SessionEnd", 1.0));
        assert!(r.sessions.is_empty());
    }

    #[test]
    fn test_session_end_inconnue_ne_cree_rien() {
        let mut r = Registre::default();
        r.appliquer(&ev("SessionEnd", 1.0));
        assert!(r.sessions.is_empty());
    }

    #[test]
    fn test_session_inconnue_creee_au_premier_evenement() {
        let mut r = Registre::default();
        r.appliquer(&outil("PreToolUse", 50.0, Some("Bash")));
        let s = &r.sessions["s1"];
        assert_eq!(s.debut, 50.0);
        assert_eq!(activite_affichee(s, 50.0, INACTIF_S), "Lance une commande");
    }

    #[test]
    fn test_session_start_sur_session_existante_garde_le_debut() {
        let mut r = registre_demarre();
        r.appliquer(&ev("SessionStart", 500.0)); // compaction, reprise
        assert_eq!(r.sessions["s1"].debut, 0.0);
    }

    #[test]
    fn test_sous_agents_prioritaires_puis_retour_au_fil_principal() {
        let mut r = registre_demarre();
        r.appliquer(&agent("SubagentStart", 1.0, "a1"));
        r.appliquer(&agent("SubagentStart", 2.0, "a2"));
        r.appliquer(&ev("Stop", 3.0)); // arrière-plan : le fil principal a fini son tour
        assert_eq!(
            activite_affichee(&r.sessions["s1"], 3.0, INACTIF_S),
            "Pilote 2 sous-agents"
        );
        r.appliquer(&agent("SubagentStop", 4.0, "a1"));
        assert_eq!(
            activite_affichee(&r.sessions["s1"], 4.0, INACTIF_S),
            "Pilote 1 sous-agent"
        );
        r.appliquer(&agent("SubagentStop", 5.0, "a2"));
        assert_eq!(
            activite_affichee(&r.sessions["s1"], 5.0, INACTIF_S),
            ATTEND_REPONSE
        );
    }

    #[test]
    fn test_evenement_de_sous_agent_ne_touche_pas_le_fil_principal() {
        let mut r = Registre::default();
        r.appliquer(&Evenement {
            cwd: Some("/p".into()),
            transcript_path: Some("/t.jsonl".into()),
            ..ev("SessionStart", 0.0)
        });
        r.appliquer(&ev("Stop", 1.0));
        r.appliquer(&Evenement {
            agent_id: Some("a1".into()),
            tool_name: Some("Edit".into()),
            cwd: Some("/autre".into()),
            transcript_path: Some("/a.jsonl".into()),
            ..ev("PreToolUse", 2.0)
        });
        let s = &r.sessions["s1"];
        assert_eq!(s.activite, ATTEND_REPONSE);
        assert_eq!(s.cwd.as_deref(), Some("/p"));
        assert_eq!(s.transcript_path.as_deref(), Some("/t.jsonl"));
        assert!(s.sous_agents.contains_key("a1")); // reconstruit sans SubagentStart
    }

    #[test]
    fn test_inactif_prioritaire_sur_les_sous_agents() {
        let mut r = Registre::default();
        r.appliquer(&agent("SubagentStart", 0.0, "a1"));
        let s = &r.sessions["s1"];
        assert_eq!(
            activite_affichee(s, 599.0, INACTIF_S),
            "Pilote 1 sous-agent"
        );
        assert_eq!(activite_affichee(s, 600.0, INACTIF_S), INACTIF);
    }

    #[test]
    fn test_evenement_de_sous_agent_repousse_l_inactivite() {
        let mut r = registre_demarre();
        r.appliquer(&ev("Stop", 1.0));
        r.appliquer(&Evenement {
            agent_id: Some("a1".into()),
            tool_name: Some("Read".into()),
            ..ev("PreToolUse", 500.0)
        });
        assert_ne!(
            activite_affichee(&r.sessions["s1"], 1000.0, INACTIF_S),
            INACTIF
        );
    }

    #[test]
    fn test_purge_des_sessions_au_pid_mort() {
        let mut r = Registre::default();
        r.appliquer(&Evenement {
            pid: Some(42),
            ..ev("SessionStart", 0.0)
        });
        r.appliquer(&Evenement {
            pid: Some(43),
            ..evs("SessionStart", 0.0, "s2")
        });
        r.purger(10.0, INACTIF_S, |pid| pid == 43);
        assert_eq!(r.sessions.keys().collect::<Vec<_>>(), vec!["s2"]);
    }

    #[test]
    fn test_purge_garde_une_session_sans_pid() {
        let mut r = registre_demarre();
        r.purger(10_000.0, INACTIF_S, |_| false);
        assert!(r.sessions.contains_key("s1"));
    }

    #[test]
    fn test_purge_retire_les_sous_agents_muets() {
        let mut r = registre_demarre();
        r.appliquer(&agent("SubagentStart", 0.0, "a1"));
        r.appliquer(&agent("SubagentStart", 500.0, "a2"));
        r.purger(650.0, INACTIF_S, |_| true);
        assert_eq!(
            r.sessions["s1"].sous_agents.keys().collect::<Vec<_>>(),
            vec!["a2"]
        );
    }

    #[test]
    fn test_purge_garde_l_ordre_des_sous_agents() {
        let mut r = registre_demarre();
        for (i, id) in ["a1", "a2", "a3", "a4"].iter().enumerate() {
            r.appliquer(&agent("SubagentStart", 400.0 + i as f64, id));
        }
        r.appliquer(&agent("SubagentStart", 0.0, "a2")); // a2 devient muet
        r.purger(700.0, INACTIF_S, |_| true);
        assert_eq!(
            r.sessions["s1"].sous_agents.keys().collect::<Vec<_>>(),
            vec!["a1", "a3", "a4"]
        );
    }

    #[test]
    fn test_session_affichee_et_debut_le_plus_ancien() {
        let mut r = Registre::default();
        r.appliquer(&evs("SessionStart", 0.0, "s1"));
        r.appliquer(&evs("SessionStart", 10.0, "s2"));
        r.appliquer(&evs("Stop", 20.0, "s1"));
        assert_eq!(r.session_affichee().unwrap().session_id, "s1");
        assert_eq!(r.debut_plus_ancien(), Some(0.0));
    }

    #[test]
    fn test_session_affichee_a_egalite_premiere_inseree() {
        let mut r = Registre::default();
        r.appliquer(&evs("SessionStart", 5.0, "s1"));
        r.appliquer(&evs("SessionStart", 5.0, "s2"));
        r.appliquer(&evs("SessionStart", 5.0, "s3"));
        assert_eq!(r.session_affichee().unwrap().session_id, "s1");
    }

    #[test]
    fn test_registre_vide() {
        let r = Registre::default();
        assert!(r.session_affichee().is_none());
        assert!(r.debut_plus_ancien().is_none());
    }

    #[test]
    fn test_pid_vivant() {
        assert!(pid_vivant(std::process::id() as i32));
        assert!(!pid_vivant((1 << 22) + 12345)); // au-delà des PID macOS
    }
}

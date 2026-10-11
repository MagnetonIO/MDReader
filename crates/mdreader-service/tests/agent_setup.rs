use mdreader_core::{ReaderStore, SourceRoot};
use mdreader_service::{
    agents::{self, Action, Agent, Scope, Selection},
    Request,
};
#[test]
fn install_is_disabled_idempotent_and_revocation_changes_authority_without_removing_user_config() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let data = temp.path().join("private");
    std::fs::create_dir(&project).unwrap();
    std::fs::create_dir(&data).unwrap();
    std::fs::write(project.join("note.md"), "# Public test\nhello").unwrap();
    std::fs::write(project.join(".mcp.json"), "{\"unrelated\":true}").unwrap();
    let root = SourceRoot::capture(project.canonicalize().unwrap()).unwrap();
    let selection = Selection {
        agent: Agent::Claude,
        scope: Scope::Project,
        root: root.path.clone(),
        identity: root.identity.clone(),
    };
    let id = selection.id().unwrap();
    let mut store = ReaderStore::open(&data).unwrap();
    let source = store.add_local_source("Public", root).unwrap();
    store.index_source(&source.id).unwrap();
    let install = Action::Install {
        selection: selection.clone(),
        executable: std::env::current_exe().unwrap(),
        client_version: "claude 2.0.0".into(),
    };
    let first = agents::execute(&mut store, &data, install.clone()).unwrap();
    assert!(first.installed);
    assert!(!first.enabled);
    assert!(first.source_ids.is_empty());
    assert!(agents::sources(&mut store, &id).is_err());
    assert!(
        agents::execute(&mut store, &data, install)
            .unwrap()
            .installed
    );
    let enabled = agents::execute(
        &mut store,
        &data,
        Action::SetMcp {
            selection: selection.clone(),
            sources: vec![source.id.clone()],
            enabled: true,
        },
    )
    .unwrap();
    assert!(enabled.enabled);
    let request = Request::Search {
        query: "hello".into(),
        sources: vec![source.id.clone()],
        limit: 10,
    };
    let generation = agents::authorize(&mut store, &id, &request).unwrap();
    assert!(agents::authorize(
        &mut store,
        &id,
        &Request::Admit {
            name: "forbidden".into(),
            path: project.clone(),
            identity: selection.identity.clone()
        }
    )
    .is_err());
    agents::execute(
        &mut store,
        &data,
        Action::SetMcp {
            selection: selection.clone(),
            sources: vec![],
            enabled: false,
        },
    )
    .unwrap();
    assert!(agents::authorize(&mut store, &id, &request).is_err());
    agents::execute(
        &mut store,
        &data,
        Action::SetMcp {
            selection: selection.clone(),
            sources: vec![source.id],
            enabled: true,
        },
    )
    .unwrap();
    assert_ne!(
        agents::authorize(&mut store, &id, &request).unwrap(),
        generation
    );
    let removed = agents::execute(&mut store, &data, Action::Uninstall { selection }).unwrap();
    assert!(!removed.installed);
    assert!(!removed.enabled);
    assert_eq!(
        std::fs::read_to_string(project.join(".mcp.json")).unwrap(),
        "{\"unrelated\":true}"
    );
    assert!(!project.join(".claude/skills/mdreader/SKILL.md").exists());
    assert!(agents::sources(&mut store, &id).is_err());
}
#[test]
fn user_scope_preserves_user_settings_and_user_authored_skill_on_conflict() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let data = temp.path().join("private");
    std::fs::create_dir(&home).unwrap();
    std::fs::create_dir(&data).unwrap();
    std::fs::create_dir_all(home.join(".claude/skills/mdreader")).unwrap();
    std::fs::write(home.join(".claude/skills/mdreader/SKILL.md"), "User skill").unwrap();
    std::fs::write(home.join(".claude.json"), "{\"theme\":\"dark\"}").unwrap();
    let root = SourceRoot::capture(home.canonicalize().unwrap()).unwrap();
    let selection = Selection {
        agent: Agent::Claude,
        scope: Scope::User,
        root: root.path,
        identity: root.identity,
    };
    let mut store = ReaderStore::open(&data).unwrap();
    assert!(agents::execute(
        &mut store,
        &data,
        Action::Install {
            selection: selection.clone(),
            executable: std::env::current_exe().unwrap(),
            client_version: "claude 2.0.0".into()
        }
    )
    .is_err());
    assert_eq!(
        std::fs::read_to_string(home.join(".claude/skills/mdreader/SKILL.md")).unwrap(),
        "User skill"
    );
    assert!(
        !agents::execute(&mut store, &data, Action::State { selection })
            .unwrap()
            .installed
    );
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.join(".claude.json")).unwrap()).unwrap();
    assert_eq!(json["theme"], "dark");
}

#[test]
fn all_client_scopes_install_known_config_and_skill_paths_without_granting_access() {
    for (agent, scope, config, skill) in [
        (
            Agent::Claude,
            Scope::Project,
            ".mcp.json",
            ".claude/skills/mdreader/SKILL.md",
        ),
        (
            Agent::Claude,
            Scope::User,
            ".claude.json",
            ".claude/skills/mdreader/SKILL.md",
        ),
        (
            Agent::Codex,
            Scope::Project,
            ".codex/config.toml",
            ".agents/skills/mdreader/SKILL.md",
        ),
        (
            Agent::Codex,
            Scope::User,
            ".codex/config.toml",
            ".agents/skills/mdreader/SKILL.md",
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let rootpath = temp.path().join("selected");
        let data = temp.path().join("private");
        std::fs::create_dir(&rootpath).unwrap();
        std::fs::create_dir(&data).unwrap();
        let root = SourceRoot::capture(rootpath.canonicalize().unwrap()).unwrap();
        let selection = Selection {
            agent,
            scope,
            root: root.path,
            identity: root.identity,
        };
        let mut store = ReaderStore::open(&data).unwrap();
        let result = agents::execute(
            &mut store,
            &data,
            Action::Install {
                selection: selection.clone(),
                executable: std::env::current_exe().unwrap(),
                client_version: "synthetic 2.0.0".into(),
            },
        )
        .unwrap();
        assert!(result.installed);
        assert!(!result.enabled);
        let text = std::fs::read_to_string(rootpath.join(config)).unwrap();
        assert!(text.contains("--grant"));
        assert!(!text.contains("--source"));
        assert!(rootpath.join(skill).is_file());
        drop(store);
        let mut store = ReaderStore::open(&data).unwrap();
        assert!(
            agents::execute(
                &mut store,
                &data,
                Action::State {
                    selection: selection.clone()
                }
            )
            .unwrap()
            .installed
        );
        agents::execute(&mut store, &data, Action::Uninstall { selection }).unwrap();
        assert!(!rootpath.join(skill).exists());
    }
}

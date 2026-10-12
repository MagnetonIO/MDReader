use mdreader_core::{ReaderError, ReaderStore, SourceRoot};
use mdreader_service::agents::{self, Action, Agent, Scope, Selection};
use std::path::Path;

const SKILL: &[u8] = include_bytes!("../agent-skill/SKILL.md");

fn selection(root: &Path, agent: Agent, scope: Scope) -> Selection {
    let root = SourceRoot::capture(root.canonicalize().unwrap()).unwrap();
    Selection {
        agent,
        scope,
        root: root.path,
        identity: root.identity,
    }
}
fn install(selection: &Selection) -> Action {
    Action::Install {
        selection: selection.clone(),
        executable: std::env::current_exe().unwrap(),
        client_version: "synthetic 2.0.0".into(),
    }
}
fn ownership(store: &mut ReaderStore) -> bool {
    store.with_library_connection(|db| {
        let text: String = db
            .query_row("SELECT record FROM main.mdreader_agent_setup", [], |r| {
                r.get(0)
            })
            .unwrap();
        serde_json::from_str::<serde_json::Value>(&text).unwrap()["skill_owned"]
            .as_bool()
            .unwrap()
    })
}

#[test]
fn externally_installed_canonical_skill_survives_config_install_reopen_and_uninstall() {
    for (agent, scope, skill, config, original) in [
        (
            Agent::Claude,
            Scope::Project,
            ".claude/skills/mdreader/SKILL.md",
            ".mcp.json",
            "{\"unrelated\":true}",
        ),
        (
            Agent::Claude,
            Scope::User,
            ".claude/skills/mdreader/SKILL.md",
            ".claude.json",
            "{\"theme\":\"dark\"}",
        ),
        (
            Agent::Codex,
            Scope::Project,
            ".agents/skills/mdreader/SKILL.md",
            ".codex/config.toml",
            "theme = \"dark\"\n",
        ),
        (
            Agent::Codex,
            Scope::User,
            ".agents/skills/mdreader/SKILL.md",
            ".codex/config.toml",
            "theme = \"dark\"\n",
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("selected");
        let data = temp.path().join("private");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&data).unwrap();
        let skill = root.join(skill);
        std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
        std::fs::create_dir_all(root.join(config).parent().unwrap()).unwrap();
        std::fs::write(&skill, SKILL).unwrap();
        std::fs::write(root.join(config), original).unwrap();
        let before = std::fs::metadata(&skill).unwrap();
        let selection = selection(&root, agent, scope);
        let mut store = ReaderStore::open(&data).unwrap();
        let state = agents::execute(&mut store, &data, install(&selection)).unwrap();
        assert!(state.installed);
        assert!(!state.enabled);
        assert!(state.source_ids.is_empty());
        assert!(!ownership(&mut store));
        assert!(
            std::fs::read_to_string(root.join(config))
                .unwrap()
                .contains("--grant")
        );
        drop(store);
        let mut store = ReaderStore::open(&data).unwrap();
        assert!(
            agents::execute(&mut store, &data, install(&selection))
                .unwrap()
                .installed
        );
        assert!(!ownership(&mut store));
        agents::execute(&mut store, &data, Action::Uninstall { selection }).unwrap();
        assert_eq!(
            std::fs::read(root.join(config)).unwrap(),
            original.as_bytes()
        );
        assert_eq!(std::fs::read(&skill).unwrap(), SKILL);
        let after = std::fs::metadata(&skill).unwrap();
        assert_eq!(before.modified().unwrap(), after.modified().unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
        }
    }
}

#[test]
fn edited_external_distribution_conflicts_without_config_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("selected");
    let data = temp.path().join("private");
    let skill = root.join(".agents/skills/mdreader/SKILL.md");
    let config = root.join(".codex/config.toml");
    std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::create_dir(&data).unwrap();
    let mut edited = SKILL.to_vec();
    edited.extend_from_slice(b"\nUser modification\n");
    std::fs::write(&skill, &edited).unwrap();
    std::fs::write(&config, "theme = \"dark\"\n").unwrap();
    let selection = selection(&root, Agent::Codex, Scope::Project);
    let mut store = ReaderStore::open(&data).unwrap();
    assert!(matches!(
        agents::execute(&mut store, &data, install(&selection)),
        Err(ReaderError::Conflict)
    ));
    assert_eq!(std::fs::read(&skill).unwrap(), edited);
    assert_eq!(
        std::fs::read_to_string(&config).unwrap(),
        "theme = \"dark\"\n"
    );
    let count = store.with_library_connection(|db| {
        db.query_row("SELECT count(*) FROM main.mdreader_agent_setup", [], |r| {
            r.get::<_, u64>(0)
        })
        .unwrap()
    });
    assert_eq!(count, 0);
}

#[test]
fn older_installation_records_keep_owned_cleanup_and_removed_records_cannot_adopt_external_skill() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("selected");
    let data = temp.path().join("private");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(&data).unwrap();
    let skill = root.join(".claude/skills/mdreader/SKILL.md");
    let selection = selection(&root, Agent::Claude, Scope::Project);
    let mut store = ReaderStore::open(&data).unwrap();
    agents::execute(&mut store, &data, install(&selection)).unwrap();
    assert!(ownership(&mut store));
    store.with_library_connection(|db| {
        let text: String = db
            .query_row("SELECT record FROM main.mdreader_agent_setup", [], |r| {
                r.get(0)
            })
            .unwrap();
        let mut record: serde_json::Value = serde_json::from_str(&text).unwrap();
        record.as_object_mut().unwrap().remove("skill_owned");
        db.execute(
            "UPDATE main.mdreader_agent_setup SET record=?1",
            [record.to_string()],
        )
        .unwrap();
    });
    drop(store);
    let mut store = ReaderStore::open(&data).unwrap();
    agents::execute(
        &mut store,
        &data,
        Action::Uninstall {
            selection: selection.clone(),
        },
    )
    .unwrap();
    assert!(!skill.exists());
    // An independently distributed copy after removal starts a new external lifecycle.
    std::fs::write(&skill, SKILL).unwrap();
    agents::execute(&mut store, &data, install(&selection)).unwrap();
    assert!(!ownership(&mut store));
    // External edits after installation must also survive MCP-only cleanup.
    std::fs::write(&skill, b"External edit after install").unwrap();
    agents::execute(&mut store, &data, Action::Uninstall { selection }).unwrap();
    assert_eq!(
        std::fs::read(&skill).unwrap(),
        b"External edit after install"
    );
}

#[test]
fn legacy_incomplete_install_cannot_claim_external_skill_on_retry_or_uninstall() {
    for retry in [false, true] {
        for phase in ["installing", "removing"] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("selected");
            let data = temp.path().join("private");
            std::fs::create_dir(&root).unwrap();
            std::fs::create_dir(&data).unwrap();
            let skill = root.join(".claude/skills/mdreader/SKILL.md");
            let selection = selection(&root, Agent::Claude, Scope::Project);
            let mut store = ReaderStore::open(&data).unwrap();
            agents::execute(&mut store, &data, install(&selection)).unwrap();
            store.with_library_connection(|db| {
                let text: String = db
                    .query_row("SELECT record FROM main.mdreader_agent_setup", [], |r| {
                        r.get(0)
                    })
                    .unwrap();
                let mut record: serde_json::Value = serde_json::from_str(&text).unwrap();
                record.as_object_mut().unwrap().remove("skill_owned");
                record["phase"] = phase.into();
                db.execute(
                    "UPDATE main.mdreader_agent_setup SET record=?1",
                    [record.to_string()],
                )
                .unwrap();
            });
            // Simulate interruption before skill creation, followed by external installation.
            std::fs::remove_file(&skill).unwrap();
            std::fs::write(&skill, SKILL).unwrap();
            let before = std::fs::metadata(&skill).unwrap();
            drop(store);
            let mut store = ReaderStore::open(&data).unwrap();
            if retry {
                agents::execute(&mut store, &data, install(&selection)).unwrap();
                assert!(!ownership(&mut store));
            }
            agents::execute(&mut store, &data, Action::Uninstall { selection }).unwrap();
            assert_eq!(std::fs::read(&skill).unwrap(), SKILL);
            assert_eq!(
                before.modified().unwrap(),
                std::fs::metadata(&skill).unwrap().modified().unwrap()
            );
        }
    }
}

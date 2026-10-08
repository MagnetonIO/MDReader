use super::*;
use std::fs;

fn owner() -> (tempfile::TempDir, JsonOwner) {
    let temporary = tempfile::tempdir().expect("temporary profile");
    let root = SourceRoot::capture(fs::canonicalize(temporary.path()).expect("canonical profile"))
        .expect("root");
    let owner = JsonOwner::new(root).expect("owner");
    (temporary, owner)
}
fn session(tabs: &str) -> String {
    format!(r#"{{"version":1,"root":"/selected","activeTabId":null,"tabs":{tabs},"recent":[]}}"#)
}
fn tab() -> &'static str {
    r#"{"id":"one","docRef":{"path":"/selected/a.md","name":"a"},"history":[{"path":"/selected/a.md","name":"a"}],"historyIndex":0,"scroll":{"block":0,"offset":0},"pinned":true,"originRoot":"/selected"}"#
}

#[test]
fn exact_preflush_unknown_values_and_latest_receipts_survive_reopen() {
    let (temporary, owner) = owner();
    let original = format!(
        " {{\"session\":{},\"external\": 1.23000000000000000000000000001e+999 }}\n",
        session(&format!(
            "[{}]",
            tab().replace(
                "\"pinned\":true",
                "\"pinned\":true,\"future\":{\"large\":900719925474099312345}"
            )
        ))
    );
    fs::write(temporary.path().join("session.json"), original.as_bytes()).expect("legacy bytes");
    fs::write(
        temporary.path().join("settings.json"),
        b"{\"settings\":{\"theme\":\"dark\",\"future\": 1.000000000000001},\"other\":true}",
    )
    .expect("settings");
    let identity = owner.load("main").expect("load").identity;
    let operation = Uuid::new_v4();
    let preflush = owner.begin_flush(operation).expect("begin");
    assert_eq!(preflush[0].bytes.as_deref(), Some(original.as_bytes()));
    assert!(matches!(
        owner.write("main", &identity, JsonKind::Session, 1, &session("[]")),
        Err(PersistenceError::Frozen)
    ));
    let session_receipt = owner
        .flush(
            operation,
            "main",
            &identity,
            JsonKind::Session,
            1,
            &session(&format!("[{}]", tab())),
        )
        .expect("flush latest");
    let settings_receipt = owner
        .flush(
            operation,
            "main",
            &identity,
            JsonKind::Settings,
            1,
            r#"{"theme":"light"}"#,
        )
        .expect("settings latest");
    owner
        .revalidate_flush(operation, &[session_receipt.clone(), settings_receipt])
        .expect("strict proof");
    owner.release_flush(operation).expect("release");
    let bytes = fs::read_to_string(temporary.path().join("session.json")).expect("persisted");
    assert!(bytes.contains("1.23000000000000000000000000001e+999"));
    assert!(bytes.contains("900719925474099312345"));
    assert!(fs::read_to_string(temporary.path().join("settings.json"))
        .expect("persisted settings")
        .contains("1.000000000000001"));
    let replay = owner
        .write(
            "main",
            &identity,
            JsonKind::Session,
            1,
            &session(&format!("[{}]", tab())),
        )
        .expect("same receipt");
    assert_eq!(replay.sha256, session_receipt.sha256);
    assert!(matches!(
        owner.write("main", &identity, JsonKind::Session, 1, &session("[]")),
        Err(PersistenceError::Conflict)
    ));
    drop(owner);
    let reopened = JsonOwner::new(
        SourceRoot::capture(fs::canonicalize(temporary.path()).expect("canonical")).expect("root"),
    )
    .expect("reopen");
    assert_eq!(
        reopened
            .load("main")
            .expect("loaded")
            .session_json
            .as_deref(),
        Some(bytes.as_str())
    );
    assert!(matches!(
        reopened.write("main", &identity, JsonKind::Session, 2, &session("[]")),
        Err(PersistenceError::Conflict)
    ));
}

#[test]
fn invalid_duplicate_future_and_bounded_inputs_never_replace_originals() {
    let (temporary, owner) = owner();
    let identity = owner.load("main").expect("identity").identity;
    let path = temporary.path().join("session.json");
    for original in [
        r#"{"session":{"version":2}}"#,
        r#"{"session":{"version":1,"tabs":[{"id":"same","id":"duplicate"}]}}"#,
        "{invalid",
    ] {
        fs::write(&path, original).expect("fixture");
        assert!(owner
            .write("main", &identity, JsonKind::Session, 1, &session("[]"))
            .is_err());
        assert_eq!(fs::read_to_string(&path).expect("untouched"), original);
    }
    fs::remove_file(&path).expect("remove fixture");
    assert!(matches!(
        owner.write(
            "main",
            &identity,
            JsonKind::Session,
            1,
            &" ".repeat(1024 * 1024 + 1)
        ),
        Err(PersistenceError::TooLarge)
    ));
    assert!(!path.exists());
    assert!(matches!(
        owner.write(
            "other-window",
            &identity,
            JsonKind::Session,
            1,
            &session("[]")
        ),
        Err(PersistenceError::Conflict)
    ));
}

#[test]
fn competing_owner_path_replacement_and_receipt_tamper_fail_closed() {
    let (temporary, owner) = owner();
    let root =
        SourceRoot::capture(fs::canonicalize(temporary.path()).expect("canonical")).expect("root");
    assert!(matches!(
        JsonOwner::new(root),
        Err(PersistenceError::Unavailable)
    ));
    let identity = owner.load("main").expect("load").identity;
    let operation = Uuid::new_v4();
    owner.begin_flush(operation).expect("begin");
    let a = owner
        .flush(
            operation,
            "main",
            &identity,
            JsonKind::Session,
            1,
            &session("[]"),
        )
        .expect("session");
    let b = owner
        .flush(
            operation,
            "main",
            &identity,
            JsonKind::Settings,
            1,
            r#"{"theme":"dark"}"#,
        )
        .expect("settings");
    fs::write(
        temporary.path().join("settings.json"),
        b"{\"settings\":{\"theme\":\"light\"}}",
    )
    .expect("external change");
    assert!(matches!(
        owner.revalidate_flush(operation, &[a, b]),
        Err(PersistenceError::Conflict)
    ));
    let displaced = temporary.path().join(".displaced-lock");
    fs::rename(
        temporary.path().join(".mdreader-json-owner.lock"),
        displaced,
    )
    .expect("replace namespace");
    fs::write(temporary.path().join(".mdreader-json-owner.lock"), []).expect("new lock");
    assert!(matches!(
        owner.release_flush(operation),
        Err(PersistenceError::Conflict)
    ));
}

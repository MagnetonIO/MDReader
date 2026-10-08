//! Derived catalog continuation epochs include cache publication and revocation.
//! Triggers participate in the existing owner transaction; no second writer.
use super::*;
const PREFIX: &str = "mdreader_catalog_provider_";
fn definitions() -> Vec<(&'static str, String)> {
    let mut items=vec![
        ("mdreader_catalog_provider_schema", "CREATE TABLE mdreader_catalog_provider_schema(singleton INTEGER PRIMARY KEY CHECK(singleton=1),version INTEGER NOT NULL)".into()),
        ("mdreader_catalog_provider_epochs", "CREATE TABLE mdreader_catalog_provider_epochs(source TEXT PRIMARY KEY,epoch TEXT NOT NULL CHECK(length(epoch)=32))".into()),
    ];
    for (name, event, table, key) in [
        (
            "mdreader_catalog_provider_cache_insert",
            "INSERT",
            "cache",
            "NEW.source",
        ),
        (
            "mdreader_catalog_provider_cache_update",
            "UPDATE",
            "cache",
            "NEW.source",
        ),
        (
            "mdreader_catalog_provider_cache_delete",
            "DELETE",
            "cache",
            "OLD.source",
        ),
        (
            "mdreader_catalog_provider_state_update",
            "UPDATE",
            "state",
            "NEW.source",
        ),
        (
            "mdreader_catalog_provider_source_insert",
            "INSERT",
            "sources",
            "NEW.identity",
        ),
        (
            "mdreader_catalog_provider_source_update",
            "UPDATE",
            "sources",
            "NEW.identity",
        ),
        (
            "mdreader_catalog_provider_source_delete",
            "DELETE",
            "sources",
            "OLD.identity",
        ),
    ] {
        items.push((name,format!("CREATE TRIGGER {name} AFTER {event} ON mdreader_provider_registry_{table} BEGIN INSERT INTO mdreader_catalog_provider_epochs VALUES({key},lower(hex(randomblob(16)))) ON CONFLICT(source) DO UPDATE SET epoch=excluded.epoch; END")));
    }
    items
}
fn normalize(value: &str) -> String {
    value
        .trim()
        .trim_end_matches(';')
        .chars()
        .filter(|c| !c.is_ascii_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}
pub(super) fn initialize(c: &mut Connection) -> Result<()> {
    registry::initialize(c)?;
    let definitions = definitions();
    let count: u32 = registry::db(c.query_row(
        "SELECT count(*) FROM sqlite_schema WHERE substr(name,1,?1)=?2",
        rusqlite::params![PREFIX.len(), PREFIX],
        |r| r.get(0),
    ))?;
    if count == 0 {
        let tx = registry::db(c.transaction())?;
        for (_, sql) in &definitions {
            registry::db(tx.execute_batch(sql))?;
        }
        registry::db(tx.execute(
            "INSERT INTO mdreader_catalog_provider_schema VALUES(1,1)",
            [],
        ))?;
        registry::db(tx.execute("INSERT INTO mdreader_catalog_provider_epochs SELECT identity,lower(hex(randomblob(16))) FROM mdreader_provider_registry_sources",[]))?;
        validate(&tx)?;
        registry::db(tx.commit())?;
    }
    validate(c)
}
fn validate(c: &Connection) -> Result<()> {
    let definitions = definitions();
    let count: u32 = registry::db(c.query_row(
        "SELECT count(*) FROM sqlite_schema WHERE substr(name,1,?1)=?2",
        rusqlite::params![PREFIX.len(), PREFIX],
        |r| r.get(0),
    ))?;
    if count as usize != definitions.len() {
        return Err(ProviderError::Unsupported);
    }
    for (name, expected) in definitions {
        let (kind, actual): (String, String) = registry::db(c.query_row(
            "SELECT type,sql FROM sqlite_schema WHERE name=?1",
            [name],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ))?;
        let expected_kind = if expected.starts_with("CREATE TABLE") {
            "table"
        } else {
            "trigger"
        };
        if kind != expected_kind || normalize(&actual) != normalize(&expected) {
            return Err(ProviderError::Unsupported);
        }
    }
    let (count,version):(u32,Option<i64>)=registry::db(c.query_row("SELECT count(*),max(CASE WHEN singleton=1 THEN version END) FROM mdreader_catalog_provider_schema",[],|r|Ok((r.get(0)?,r.get(1)?))))?;
    if count != 1 || version != Some(1) {
        return Err(ProviderError::Unsupported);
    }
    Ok(())
}
pub(super) fn epoch(c: &mut Connection, b: &SourceBinding) -> Result<String> {
    initialize(c)?;
    registry::descriptor_at(c, b, true)?;
    let epoch: String = registry::db(c.query_row(
        "SELECT epoch FROM mdreader_catalog_provider_epochs WHERE source=?1",
        [registry::identity(b)],
        |r| r.get(0),
    ))?;
    if epoch.len() != 32 || !epoch.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ProviderError::Persistence);
    }
    Ok(epoch)
}

//! Metadata-only project reporting and explicitly granted revision-bound writes.
use super::*;
use mdreader_service::agents::projects::AgentRequest;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegisterSession {
    #[serde(default)]
    project_id: Option<String>,
    client_session_id: String,
    title: String,
    #[serde(default)]
    cwd: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegisterDocument {
    session_id: String,
    #[serde(default)]
    relative_path: Option<String>,
    #[serde(default)]
    path: Option<String>,
}
/// An absolute, NUL-free path within budget.
fn absolute(value: &str) -> bool {
    value.len() <= 4096 && !value.contains('\0') && Path::new(value).is_absolute()
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EndSession {
    session_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Create {
    source_id: String,
    path: String,
    operation_id: String,
    markdown: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Update {
    source_id: String,
    path: String,
    operation_id: String,
    expected_revision: String,
    markdown: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Inspect {
    operation_id: String,
}
fn id(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok()
}
fn operation(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|v| v.get_version_num() == 4)
}
fn text(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}
pub(super) fn call(config: &Config, name: &str, args: Value) -> Result<Value, Failure> {
    let request = match name {
        "list_projects" => {
            closed(&args, &[])?;
            AgentRequest::ListProjects
        }
        "register_session" => {
            let a: RegisterSession = decode(args)?;
            if a.project_id.as_deref().is_some_and(|p| !valid_id(p))
                || a.cwd.as_deref().is_some_and(|c| !absolute(c))
                || !text(&a.client_session_id, 256)
                || !text(&a.title, 256)
            {
                return Err(Failure::Params);
            }
            AgentRequest::RegisterSession {
                project_id: a.project_id,
                client_session_id: a.client_session_id,
                title: a.title,
                cwd: a.cwd.map(Into::into),
            }
        }
        "register_document" => {
            let a: RegisterDocument = decode(args)?;
            let one = match (&a.relative_path, &a.path) {
                (Some(relative), None) => valid_path(relative),
                (None, Some(path)) => absolute(path),
                _ => false,
            };
            if !id(&a.session_id) || !one {
                return Err(Failure::Params);
            }
            AgentRequest::RegisterDocument {
                session_id: a.session_id,
                relative_path: a.relative_path,
                path: a.path.map(Into::into),
            }
        }
        "end_session" => {
            let a: EndSession = decode(args)?;
            if !id(&a.session_id) {
                return Err(Failure::Params);
            }
            AgentRequest::EndSession {
                session_id: a.session_id,
            }
        }
        "create_document" => {
            let a: Create = decode(args)?;
            if !valid_id(&a.source_id)
                || !valid_path(&a.path)
                || !operation(&a.operation_id)
                || a.markdown.len() > 1024 * 1024
            {
                return Err(Failure::Params);
            }
            AgentRequest::CreateDocument {
                source_id: a.source_id,
                path: a.path,
                operation_id: a.operation_id,
                markdown: a.markdown,
            }
        }
        "update_document" => {
            let a: Update = decode(args)?;
            if !valid_id(&a.source_id)
                || !valid_path(&a.path)
                || !operation(&a.operation_id)
                || !valid_revision(&a.expected_revision)
                || a.markdown.len() > 1024 * 1024
            {
                return Err(Failure::Params);
            }
            AgentRequest::UpdateDocument {
                source_id: a.source_id,
                path: a.path,
                operation_id: a.operation_id,
                expected_revision: a.expected_revision,
                markdown: a.markdown,
            }
        }
        "inspect_operation" => {
            let a: Inspect = decode(args)?;
            if !operation(&a.operation_id) {
                return Err(Failure::Params);
            }
            AgentRequest::InspectOperation {
                operation_id: a.operation_id,
            }
        }
        _ => return Err(Failure::Params),
    };
    config.agent_call(request)
}
pub(super) fn tools() -> Vec<Value> {
    let id = json!({"type":"string","format":"uuid"});
    let source = string_schema(128);
    let path = string_schema(4096);
    let body = json!({"type":"string","maxLength":1024*1024});
    // (name, description, properties, required, readOnly, destructive, idempotent)
    let specs=[
        ("list_projects","List only selected reporting projects; no folders are admitted.",json!({}),json!([]),true,false,true),
        ("register_session","Register/resume session metadata: a stable client session ID, a short title and optionally the absolute working directory (or a selected project). No chat content is retained.",json!({"project_id":source,"client_session_id":string_schema(256),"title":string_schema(256),"cwd":string_schema(4096)}),json!(["client_session_id","title"]),false,false,true),
        ("register_document","Associate an existing Markdown file the session created or edited: an absolute `path`, or a `relative_path` under the session's working directory.",json!({"session_id":id,"relative_path":path,"path":string_schema(4096)}),json!(["session_id"]),false,false,true),
        ("end_session","Mark a reporting session ended; documents and metadata remain available.",json!({"session_id":id}),json!(["session_id"]),false,false,true),
        ("create_document","Create Markdown only in an explicitly editable source. Refuses an existing destination. Keep the operation ID for recovery.",json!({"source_id":source,"path":path,"operation_id":id,"markdown":body}),json!(["source_id","path","operation_id","markdown"]),false,false,false),
        ("update_document","Update an explicitly editable document using its exact expected revision. Conflicts never force overwrite; pending saves can be inspected.",json!({"source_id":source,"path":path,"operation_id":id,"expected_revision":string_schema(64),"markdown":body}),json!(["source_id","path","operation_id","expected_revision","markdown"]),false,true,false),
        ("inspect_operation","Inspect the connection's retained pending or completed document operation without resending it.",json!({"operation_id":id}),json!(["operation_id"]),true,false,true),
    ];
    specs.into_iter().map(|(name,description,properties,required,read,destructive,idempotent)|json!({"name":name,"description":description,"inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false},"annotations":{"readOnlyHint":read,"destructiveHint":destructive,"idempotentHint":idempotent,"openWorldHint":false}})).collect()
}

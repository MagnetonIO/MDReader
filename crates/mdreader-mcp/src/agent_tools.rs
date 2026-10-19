//! Metadata-only project reporting and explicitly granted revision-bound writes.
use super::*;
use mdreader_service::agents::projects::AgentRequest;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegisterSession {
    project_id: String,
    client_session_id: String,
    title: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegisterDocument {
    session_id: String,
    relative_path: String,
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
            if !valid_id(&a.project_id) || !text(&a.client_session_id, 256) || !text(&a.title, 256)
            {
                return Err(Failure::Params);
            }
            AgentRequest::RegisterSession {
                project_id: a.project_id,
                client_session_id: a.client_session_id,
                title: a.title,
            }
        }
        "register_document" => {
            let a: RegisterDocument = decode(args)?;
            if !id(&a.session_id) || !valid_path(&a.relative_path) {
                return Err(Failure::Params);
            }
            AgentRequest::RegisterDocument {
                session_id: a.session_id,
                relative_path: a.relative_path,
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
        ("register_session","Register/resume session metadata in a selected project. No chat content is retained.",json!({"project_id":source,"client_session_id":string_schema(256),"title":string_schema(256)}),json!(["project_id","client_session_id","title"]),false,false,true),
        ("register_document","Associate an existing project-relative Markdown file with the reporting session; folder watching indexes its content.",json!({"session_id":id,"relative_path":path}),json!(["session_id","relative_path"]),false,false,true),
        ("end_session","Mark a reporting session ended; documents and metadata remain available.",json!({"session_id":id}),json!(["session_id"]),false,false,true),
        ("create_document","Create Markdown only in an explicitly editable source. Refuses an existing destination. Keep the operation ID for recovery.",json!({"source_id":source,"path":path,"operation_id":id,"markdown":body}),json!(["source_id","path","operation_id","markdown"]),false,false,false),
        ("update_document","Update an explicitly editable document using its exact expected revision. Conflicts never force overwrite; pending saves can be inspected.",json!({"source_id":source,"path":path,"operation_id":id,"expected_revision":string_schema(64),"markdown":body}),json!(["source_id","path","operation_id","expected_revision","markdown"]),false,true,false),
        ("inspect_operation","Inspect the connection's retained pending or completed document operation without resending it.",json!({"operation_id":id}),json!(["operation_id"]),true,false,true),
    ];
    specs.into_iter().map(|(name,description,properties,required,read,destructive,idempotent)|json!({"name":name,"description":description,"inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false},"annotations":{"readOnlyHint":read,"destructiveHint":destructive,"idempotentHint":idempotent,"openWorldHint":false}})).collect()
}

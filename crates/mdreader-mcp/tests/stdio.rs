use mdreader_core::SourceRoot;
use mdreader_service::{call, Request};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use tempfile::{tempdir, TempDir};

struct Daemon(Child);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
// The real service runs in another OS process without requiring an installed
// executable or touching a user's library. Only this ignored fixture uses env.
#[test]
#[ignore]
fn service_fixture() {
    let data = std::env::var_os("MDREADER_SYNTHETIC_SERVICE_DIR").unwrap();
    mdreader_service::serve(Path::new(&data)).unwrap();
}
fn service(data: &Path) -> Daemon {
    let mut process = Daemon(
        Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "service_fixture", "--nocapture"])
            .env("MDREADER_SYNTHETIC_SERVICE_DIR", data)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    for _ in 0..200 {
        if call(data, &Request::Ping).is_ok() {
            return process;
        }
        assert!(process.0.try_wait().unwrap().is_none(), "service exited");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("synthetic service startup timed out");
}
struct Client {
    process: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
}
impl Client {
    fn new(data: &Path, sources: &[&str]) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mdreader-mcp"));
        command.arg("--data-dir").arg(data);
        for source in sources {
            command.arg("--source").arg(source);
        }
        let mut process = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        Self {
            input: process.stdin.take(),
            output: BufReader::new(process.stdout.take().unwrap()),
            process,
        }
    }
    fn grant(data: &Path, grant: &str) -> Self {
        let mut process = Command::new(env!("CARGO_BIN_EXE_mdreader-mcp"))
            .arg("--data-dir")
            .arg(data)
            .arg("--grant")
            .arg(grant)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        Self {
            input: process.stdin.take(),
            output: BufReader::new(process.stdout.take().unwrap()),
            process,
        }
    }
    fn send(&mut self, value: Value) {
        writeln!(self.input.as_mut().unwrap(), "{value}").unwrap();
        self.input.as_mut().unwrap().flush().unwrap();
    }
    fn receive(&mut self) -> Value {
        let mut line = String::new();
        assert!(
            self.output.read_line(&mut line).unwrap() > 0,
            "MCP closed stdout"
        );
        serde_json::from_str(&line).unwrap()
    }
    fn request(&mut self, id: Value, method: &str, params: Value) -> Value {
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
        let response = self.receive();
        assert_eq!(response["id"], id);
        response
    }
    fn ready(&mut self) {
        let init = self.request(json!(1), "initialize", json!({
            "protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"synthetic","version":"1"}}));
        assert_eq!(init["result"]["protocolVersion"], "2025-11-25");
        self.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    }
    fn tool(&mut self, name: &str, args: Value) -> Value {
        self.request(
            json!("tool"),
            "tools/call",
            json!({"name":name,"arguments":args}),
        )
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.input.take();
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}
struct Fixture {
    data: TempDir,
    notes: TempDir,
    other: TempDir,
    daemon: Option<Daemon>,
    id: String,
    hidden: String,
}
impl Fixture {
    fn new() -> Self {
        let data = tempdir().unwrap();
        let notes = tempdir().unwrap();
        let other = tempdir().unwrap();
        std::fs::write(
            notes.path().join("plan.md"),
            "# Plan\nDurable mcp evidence.\n\n[Next](next.md#next)\n",
        )
        .unwrap();
        std::fs::write(
            notes.path().join("next.md"),
            "# Next\nMore durable evidence.\n",
        )
        .unwrap();
        std::fs::write(
            other.path().join("secret.md"),
            "# Secret\nDurable private secret.\n",
        )
        .unwrap();
        let daemon = Some(service(data.path()));
        let admit = |path: &Path, name: &str| {
            let root = SourceRoot::capture(path.canonicalize().unwrap()).unwrap();
            let value = call(
                data.path(),
                &Request::Admit {
                    name: name.into(),
                    path: root.path,
                    identity: root.identity,
                },
            )
            .unwrap();
            let id = value["id"].as_str().unwrap().to_owned();
            call(
                data.path(),
                &Request::Index {
                    source_id: id.clone(),
                },
            )
            .unwrap();
            id
        };
        let id = admit(notes.path(), "Selected");
        let hidden = admit(other.path(), "PRIVATE_SOURCE_NAME");
        Self {
            data,
            notes,
            other,
            daemon,
            id,
            hidden,
        }
    }
}
fn data(response: &Value) -> Value {
    assert_eq!(response["result"]["isError"], false, "{response}");
    let value = response["result"]["structuredContent"]["data"].clone();
    assert_eq!(
        serde_json::from_str::<Value>(
            response["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .strip_prefix(&format!("{}\n", mdreader_capture::RETRIEVAL_MARKER))
                .unwrap()
        )
        .unwrap(),
        response["result"]["structuredContent"]
    );
    value
}

#[test]
fn lifecycle_negotiation_ids_notifications_and_closed_schemas() {
    let fixture = Fixture::new();
    let mut client = Client::new(fixture.data.path(), &[&fixture.id]);
    assert_eq!(
        client.request(json!(0), "tools/list", json!({}))["error"]["code"],
        -32002
    );
    assert_eq!(
        client.request(json!("ping"), "ping", json!({}))["result"],
        json!({})
    );
    let init = client.request(json!(2), "initialize", json!({"protocolVersion":"future","capabilities":{},"clientInfo":{"name":"test","version":"1"}}));
    assert_eq!(init["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(
        client.request(json!(3), "tools/list", json!({}))["error"]["code"],
        -32002
    );
    client.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    client.send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":"unknown"}}));
    client.send(json!({"jsonrpc":"2.0","method":"unknown_notification"}));
    let tools = client.request(json!(4), "tools/list", json!({}));
    let tools = tools["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 13);
    assert_eq!(
        tools
            .iter()
            .map(|x| x["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "list_sources",
            "index_status",
            "search",
            "read_document",
            "related",
            "resolve_reference",
            "list_projects",
            "register_session",
            "register_document",
            "end_session",
            "create_document",
            "update_document",
            "inspect_operation"
        ]
    );
    for tool in tools {
        assert_eq!(
            tool["annotations"]["readOnlyHint"],
            !matches!(
                tool["name"].as_str().unwrap(),
                "register_session"
                    | "register_document"
                    | "end_session"
                    | "create_document"
                    | "update_document"
            )
        );
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);
    }
    assert_eq!(
        client.request(json!(5), "missing", json!({}))["error"]["code"],
        -32601
    );
    assert_eq!(client.tool("admit", json!({}))["error"]["code"], -32602);
    assert_eq!(
        client.tool("search", json!({"query":"Durable","path":"/private"}))["error"]["code"],
        -32602
    );
    assert_eq!(
        client.tool("search", json!({"query":"Durable","limit":101}))["error"]["code"],
        -32602
    );
    assert_eq!(
        client.request(json!(6), "initialize", json!({}))["error"]["code"],
        -32600
    );
}
#[test]
fn selected_sources_bound_all_tools_with_revision_and_line_evidence() {
    let fixture = Fixture::new();
    let mut client = Client::new(fixture.data.path(), &[&fixture.id]);
    client.ready();
    let sources = data(&client.tool("list_sources", json!({})));
    assert_eq!(sources.as_array().unwrap().len(), 1);
    assert_eq!(sources[0]["id"], fixture.id);
    let status = data(&client.tool("index_status", json!({})));
    assert_eq!(status["documents"], 2);
    let hits = data(&client.tool("search", json!({"query":"Durable","limit":1})));
    assert_eq!(hits.as_array().unwrap().len(), 1);
    assert_eq!(hits[0]["source_id"], fixture.id);
    assert!(!hits[0]["revision"].as_str().unwrap().is_empty());
    assert!(hits[0]["line_start"].as_u64().unwrap() > 0);
    let read = data(&client.tool(
        "read_document",
        json!({"source_id":fixture.id,"path":"plan.md"}),
    ));
    assert_eq!(read["line_start"], 1);
    assert_eq!(read["line_end"], 4);
    let resolved = data(&client.tool(
        "resolve_reference",
        json!({"source_id":fixture.id,"from":"plan.md","target":"next.md#next"}),
    ));
    assert_eq!(resolved[0]["path"], "next.md");
    assert_eq!(resolved[0]["line"], 1);
    let related = data(&client.tool("related", json!({"source_id":fixture.id,"path":"next.md"})));
    assert!(related
        .as_array()
        .unwrap()
        .iter()
        .all(|x| x["source_id"] == fixture.id));
    for (tool, args) in [
        ("index_status", json!({"sources":[fixture.hidden]})),
        (
            "search",
            json!({"query":"Durable","sources":[fixture.hidden]}),
        ),
        (
            "read_document",
            json!({"source_id":fixture.hidden,"path":"secret.md"}),
        ),
        (
            "related",
            json!({"source_id":fixture.hidden,"path":"secret.md"}),
        ),
        (
            "resolve_reference",
            json!({"source_id":fixture.hidden,"from":"secret.md","target":"secret.md"}),
        ),
    ] {
        let response = client.tool(tool, args);
        assert_eq!(response["result"]["isError"], true, "{response}");
        assert!(!response.to_string().contains("PRIVATE_SOURCE_NAME"));
        assert!(!response
            .to_string()
            .contains(fixture.other.path().to_str().unwrap()));
    }
    for args in [
        json!({"query":"Durable","sources":[]}),
        json!({"query":"Durable","sources":null}),
        json!({"query":"Durable","sources":[fixture.id,fixture.id]}),
    ] {
        assert_eq!(client.tool("search", args)["error"]["code"], -32602);
    }
    let missing = data(&client.tool(
        "resolve_reference",
        json!({"source_id":fixture.id,"from":"plan.md","target":"missing.md"}),
    ));
    assert_eq!(missing, json!([]));
    for path in ["../secret.md", "/absolute.md", "C:\\private.md"] {
        assert_eq!(
            client.tool("read_document", json!({"source_id":fixture.id,"path":path}))["error"]
                ["code"],
            -32602
        );
    }
}
#[test]
fn stale_revision_service_down_and_restart_are_safe_and_recoverable() {
    let mut fixture = Fixture::new();
    let mut client = Client::new(fixture.data.path(), &[&fixture.id]);
    client.ready();
    let doc = data(&client.tool(
        "read_document",
        json!({"source_id":fixture.id,"path":"plan.md"}),
    ));
    std::fs::write(
        fixture.notes.path().join("plan.md"),
        "# Plan\nChanged revision.\n",
    )
    .unwrap();
    let stale = client.tool(
        "read_document",
        json!({"source_id":fixture.id,"path":"plan.md","revision":doc["revision"]}),
    );
    assert_eq!(stale["result"]["isError"], true);
    assert!(stale.to_string().contains("stale_revision"));
    fixture.daemon.take();
    let down = client.tool("list_sources", json!({}));
    assert_eq!(down["result"]["isError"], true);
    assert!(!down
        .to_string()
        .contains(fixture.data.path().to_str().unwrap()));
    fixture.daemon = Some(service(fixture.data.path()));
    assert_eq!(
        data(&client.tool("list_sources", json!({})))[0]["id"],
        fixture.id
    );
    drop(client);
    let mut restarted = Client::new(fixture.data.path(), &[&fixture.id]);
    restarted.ready();
    assert_eq!(
        data(&restarted.tool("list_sources", json!({})))[0]["id"],
        fixture.id
    );
}
#[test]
fn malformed_input_invalid_ids_and_oversized_frames_are_bounded() {
    let data_dir = tempdir().unwrap();
    let mut client = Client::new(data_dir.path(), &["selected"]);
    writeln!(client.input.as_mut().unwrap(), "{{broken").unwrap();
    assert_eq!(client.receive()["error"]["code"], -32700);
    for value in [
        json!([]),
        json!({"jsonrpc":"2.0","id":null,"method":"ping"}),
        json!({"jsonrpc":"2.0","id":false,"method":"ping"}),
        json!({"jsonrpc":"1.0","id":1,"method":"ping"}),
    ] {
        client.send(value);
        let response = client.receive();
        assert_eq!(response["error"]["code"], -32600);
        assert!(response["id"].is_null());
    }
    client
        .input
        .as_mut()
        .unwrap()
        .write_all(&vec![b'x'; 2 * 1024 * 1024 + 2])
        .unwrap();
    assert_eq!(client.receive()["error"]["code"], -32700);
    client.input.take();
    assert!(client.process.wait().unwrap().success());
}
#[test]
fn cli_requires_explicit_scope_and_eof_exits_cleanly() {
    let directory = tempdir().unwrap();
    for args in [
        vec![],
        vec!["--data-dir", directory.path().to_str().unwrap()],
        vec!["--source", "selected"],
        vec!["--data-dir", "relative", "--source", "selected"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_mdreader-mcp"))
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!output
            .stderr
            .windows(directory.path().as_os_str().len())
            .any(|bytes| bytes == directory.path().to_str().unwrap().as_bytes()));
    }
    let mut client = Client::new(directory.path(), &["selected"]);
    client.input.take();
    assert!(client.process.wait().unwrap().success());
}

#[test]
fn incomplete_frames_invalid_utf8_and_oversized_output_fail_without_echoing_content() {
    let directory = tempdir().unwrap();
    let mut truncated = Client::new(directory.path(), &["selected"]);
    truncated
        .input
        .as_mut()
        .unwrap()
        .write_all(b"{\"jsonrpc\":")
        .unwrap();
    truncated.input.take();
    assert_eq!(truncated.receive()["error"]["code"], -32700);
    assert!(truncated.process.wait().unwrap().success());
    let mut utf8 = Client::new(directory.path(), &["selected"]);
    utf8.input
        .as_mut()
        .unwrap()
        .write_all(&[0xff, b'\n'])
        .unwrap();
    assert_eq!(utf8.receive()["error"]["code"], -32700);
    assert_eq!(
        utf8.request(json!(9), "ping", json!({}))["result"],
        json!({})
    );
    let fixture = Fixture::new();
    // A valid bounded 2 MiB Markdown document can expand past the MCP budget
    // when JSON-escaped twice (structured content plus compatibility text).
    std::fs::write(
        fixture.notes.path().join("large.md"),
        vec![1u8; 2 * 1024 * 1024],
    )
    .unwrap();
    let mut client = Client::new(fixture.data.path(), &[&fixture.id]);
    client.ready();
    let result = client.tool(
        "read_document",
        json!({"source_id":fixture.id,"path":"large.md"}),
    );
    assert_eq!(result["result"]["isError"], true);
    assert!(result.to_string().len() < 1024);
    assert_eq!(
        client.request(json!(10), "ping", json!({}))["result"],
        json!({})
    );
}

#[test]
fn returned_source_scope_is_verified_and_service_errors_are_sanitized() {
    let directory = tempdir().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    std::fs::write(directory.path().join("reader-endpoint.json"), json!({
        "protocol":1,"port":listener.local_addr().unwrap().port(),"token":"synthetic-token","pid":std::process::id()
    }).to_string()).unwrap();
    let responses = vec![
        json!([{"id":"hidden","name":"PRIVATE_SOURCE_NAME","enabled":true},{"id":"selected","name":"Selected","enabled":true,"path":"PRIVATE_ROOT"}]),
        json!([{"source_id":"hidden","path":"secret.md","revision":"a".repeat(64),"title":"Secret","heading":"Secret","line_start":1,"line_end":2,"text":"PRIVATE_CONTENT","score":1.0}]),
        json!({"source_id":"hidden","path":"plan.md","revision":"a".repeat(64),"markdown":"PRIVATE_CONTENT"}),
        json!([{"source_id":"hidden","path":"secret.md","revision":"a".repeat(64),"line":1}]),
        json!([{"source_id":"hidden","path":"secret.md","revision":"a".repeat(64),"line":1}]),
    ];
    let server = std::thread::spawn(move || {
        for response in responses {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["token"], "synthetic-token");
            writeln!(stream, "{}", json!({"result":response,"error":null})).unwrap();
        }
        let (mut stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut line)
            .unwrap();
        let request: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(request["request"]["params"]["sources"], json!(["selected"]));
        writeln!(stream,"{}",json!({"result":null,"error":{"code":"invalid_request","message":"PRIVATE_ROOT PRIVATE_TOKEN"}})).unwrap();
    });
    let mut client = Client::new(directory.path(), &["selected"]);
    client.ready();
    let sources = data(&client.tool("list_sources", json!({})));
    assert_eq!(
        sources,
        json!([{"id":"selected","name":"Selected","enabled":true}])
    );
    for (tool, args) in [
        ("search", json!({"query":"secret"})),
        (
            "read_document",
            json!({"source_id":"selected","path":"plan.md"}),
        ),
        ("related", json!({"source_id":"selected","path":"plan.md"})),
        (
            "resolve_reference",
            json!({"source_id":"selected","from":"plan.md","target":"secret.md"}),
        ),
        ("index_status", json!({})),
    ] {
        let response = client.tool(tool, args);
        assert_eq!(response["result"]["isError"], true, "{response}");
        assert!(!response.to_string().contains("PRIVATE_"));
        assert!(!response.to_string().contains("hidden"));
    }
    server.join().unwrap();
}

#[cfg(unix)]
#[test]
fn actual_mcp_display_and_structured_echoes_are_excluded_without_changing_revision_evidence() {
    use mdreader_capture::{
        Adapter, CaptureOrigin, EnqueueOutcome, Limits, ProjectPolicy, SkipReason, Spool,
        RETRIEVAL_MARKER,
    };
    let fixture = Fixture::new();
    let captures = tempdir().unwrap();
    let project = fixture.notes.path().canonicalize().unwrap();
    let policy = ProjectPolicy {
        enabled: true,
        project_id: "mcp-echo-fixture".into(),
        project_root: project.clone(),
        repository_root: project.clone(),
        worktree_roots: vec![project.clone()],
        capture_subagents: false,
        excluded_paths: vec![],
        excluded_sessions: vec![],
        selected_backfill_sessions: vec![],
    };
    let spool = Spool::open(captures.path().join("spool"), Limits::default()).unwrap();
    let mut client = Client::new(fixture.data.path(), &[&fixture.id]);
    client.ready();
    let original = std::fs::read_to_string(project.join("plan.md")).unwrap();
    let service_read = call(
        fixture.data.path(),
        &Request::Read {
            source_id: fixture.id.clone(),
            path: "plan.md".into(),
            revision: None,
        },
    )
    .unwrap();
    let response = client.tool(
        "read_document",
        json!({"source_id":fixture.id,"path":"plan.md","revision":service_read["revision"]}),
    );
    assert_eq!(response["result"]["isError"], false);
    assert_eq!(
        response["result"]["structuredContent"]["data"]["markdown"],
        original
    );
    assert_eq!(
        response["result"]["structuredContent"]["data"]["revision"],
        service_read["revision"]
    );
    assert_eq!(
        response["result"]["structuredContent"]["data"]["line_start"],
        1
    );
    assert_eq!(
        response["result"]["structuredContent"]["data"]["line_end"],
        4
    );
    let search = client.tool("search", json!({"query":"Durable","limit":1}));
    assert_eq!(search["result"]["isError"], false);
    let service_hits = call(
        fixture.data.path(),
        &Request::Search {
            query: "Durable".into(),
            sources: vec![fixture.id.clone()],
            limit: 1,
        },
    )
    .unwrap();
    assert_eq!(
        search["result"]["structuredContent"]["data"][0],
        service_hits[0]
    );
    for result in [&response["result"], &search["result"]] {
        let display = result["content"][0]["text"].as_str().unwrap();
        assert!(
            display.starts_with(&format!("{RETRIEVAL_MARKER}\n")),
            "actual MCP display must be marked"
        );
        assert_eq!(
            result["structuredContent"]["_meta"]["mdreader/retrieval"],
            RETRIEVAL_MARKER
        );
        for body in [display.to_owned(), result["structuredContent"].to_string()] {
            for adapter in [
                Adapter::ClaudeCode,
                Adapter::CodexHook,
                Adapter::CodexNotify,
            ] {
                let event = match adapter {
                    Adapter::CodexNotify => {
                        json!({"type":"agent-turn-complete","thread-id":"echo-session","turn-id":"echo-turn","cwd":project,"last-assistant-message":body})
                    }
                    _ => {
                        json!({"hook_event_name":"Stop","session_id":"echo-session","turn_id":"echo-turn","prompt_id":"echo-turn","cwd":project,"last_assistant_message":body})
                    }
                };
                assert!(matches!(
                    spool
                        .enqueue(
                            adapter,
                            &serde_json::to_vec(&event).unwrap(),
                            &policy,
                            CaptureOrigin::Live
                        )
                        .unwrap(),
                    EnqueueOutcome::Skipped {
                        reason: SkipReason::RetrievalEcho
                    }
                ));
            }
        }
    }
    assert!(spool.pending(10).unwrap().is_empty());
    // Genuine newly authored visible output remains capturable. The marker is
    // explicit provenance, never a heuristic based on words or similarity.
    let event = json!({"hook_event_name":"Stop","session_id":"new-session","turn_id":"new-turn","prompt_id":"new-turn","cwd":project,"last_assistant_message":"# New plan\nA newly authored follow-up with independent decisions.\n"});
    assert!(matches!(
        spool
            .enqueue(
                Adapter::ClaudeCode,
                &serde_json::to_vec(&event).unwrap(),
                &policy,
                CaptureOrigin::Live
            )
            .unwrap(),
        EnqueueOutcome::Queued { .. }
    ));
}

#[test]
fn explicit_search_intent_is_honest_when_semantic_model_is_unavailable() {
    let fixture = Fixture::new();
    let mut client = Client::new(fixture.data.path(), &[&fixture.id]);
    client.ready();
    let semantic = client.request(
        json!(20),
        "tools/call",
        json!({"name":"search","arguments":{"query":"durable","intent":"semantic"}}),
    );
    assert!(data(&semantic).as_array().unwrap().is_empty());
    assert!(
        semantic["result"]["structuredContent"]["_meta"]["mdreader/semanticUnavailable"]
            .is_string()
    );
    let best = client.request(
        json!(21),
        "tools/call",
        json!({"name":"search","arguments":{"query":"durable","intent":"best"}}),
    );
    assert!(!data(&best).as_array().unwrap().is_empty());
    assert!(best["result"]["_meta"]["mdreader/semanticUnavailable"].is_string());
    let exact = client.request(
        json!(22),
        "tools/call",
        json!({"name":"search","arguments":{"query":"durable","intent":"exact"}}),
    );
    assert!(!data(&exact).as_array().unwrap().is_empty());
    assert!(exact["result"]["_meta"]["mdreader/semanticUnavailable"].is_null());
    let recent = client.request(
        json!(23),
        "tools/call",
        json!({"name":"search","arguments":{"query":"durable","intent":"recent"}}),
    );
    assert!(!data(&recent).as_array().unwrap().is_empty());
    assert!(recent["result"]["_meta"]["mdreader/semanticUnavailable"].is_null());
}

#[cfg(unix)]
#[test]
fn scoped_provider_projection_reads_preserve_exact_evidence_and_revoke() {
    use mdreader_core::{ProviderProjection, ReaderStore};
    let temporary = tempdir().unwrap();
    let directory = temporary.path().join("data");
    let mut store = ReaderStore::open(&directory).unwrap();
    let projection = ProviderProjection::open(
        &temporary.path().join("private-provider-cache"),
        serde_json::from_value(json!("synthetic-provider-library")).unwrap(),
    )
    .unwrap();
    let binding = serde_json::from_value(json!({
        "library_id":"synthetic-provider-library","source_id":"synthetic-drive",
        "account":"synthetic-account","generation":"5dd6fe86-d490-4acb-847d-552eae66fa75",
        "selection_digest":"1".repeat(64)
    }))
    .unwrap();
    let version =
        json!({"provider_revision":"remote-v1","content_hash":null,"conditional_write_token":null});
    let markdown = "# Provider evidence\nSynthetic photovoltaic plan.\n";
    let metadata=serde_json::from_value(json!({"key":"opaque-key","name":"Synthetic provider note","display_path":"Notes / Synthetic","media_type":null,"modified_at_ms":1234,"size_bytes":markdown.len(),"version":version})).unwrap();
    let document=serde_json::from_value(json!({"key":"opaque-key","version":version,"body":markdown.as_bytes(),"observed_at_ms":1235})).unwrap();
    let receipt = store.with_library_connection(|connection| {
        let tx = connection.transaction().unwrap();
        let receipt = projection
            .admit(&tx, &binding, &metadata, &document)
            .unwrap();
        tx.commit().unwrap();
        receipt
    });
    assert!(
        mdreader_mcp::Config::new(directory.clone(), vec![receipt.reader_source_id.clone()])
            .is_ok(),
        "canonical provider source IDs must be selectable"
    );
    for invalid in [
        "provider:short".to_owned(),
        format!("provider:{}", "A".repeat(64)),
        format!("other:{}", "a".repeat(64)),
        "file:/private".into(),
    ] {
        assert!(mdreader_mcp::Config::new(directory.clone(), vec![invalid]).is_err());
    }
    store.index_source(&receipt.reader_source_id).unwrap();
    drop(store);
    let _daemon = service(&directory);
    let mut client = Client::new(&directory, &[&receipt.reader_source_id]);
    client.ready();
    let tools = client.request(json!("schema"), "tools/list", json!({}));
    let read_schema = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "read_document")
        .unwrap();
    assert!(
        read_schema["inputSchema"]["properties"]["source_id"]["pattern"]
            .as_str()
            .unwrap()
            .contains("provider:")
    );
    let result=data(&client.tool("read_document",json!({"source_id":receipt.reader_source_id,"path":receipt.path,"revision":receipt.revision})));
    assert_eq!(result["markdown"], markdown);
    assert_eq!(result["revision"], receipt.revision);
    assert_eq!(result["line_start"], 1);
    assert_eq!(result["line_end"], 2);
    let hits = data(&client.tool("search", json!({"query":"photovoltaic"})));
    assert!(!hits.as_array().unwrap().is_empty());
    assert!(hits
        .as_array()
        .unwrap()
        .iter()
        .all(|hit| hit["source_id"] == receipt.reader_source_id
            && hit["revision"] == receipt.revision));
    let hidden = format!("provider:{}", "0".repeat(64));
    let response = client.tool(
        "read_document",
        json!({"source_id":hidden,"path":receipt.path}),
    );
    assert_eq!(response["result"]["isError"], true);
    assert!(!response.to_string().contains("photovoltaic"));
    call(
        &directory,
        &Request::SetSourceEnabled {
            source_id: receipt.reader_source_id.clone(),
            enabled: false,
        },
    )
    .unwrap();
    let response=client.tool("read_document",json!({"source_id":receipt.reader_source_id,"path":receipt.path,"revision":receipt.revision}));
    assert_eq!(response["result"]["isError"], true);
    assert!(!response.to_string().contains("photovoltaic"));
}

#[test]
fn existing_stdio_session_observes_enable_disable_and_removed_sources() {
    use mdreader_service::agents::{Action, Agent, Scope, Selection};
    let fixture = Fixture::new();
    let root = SourceRoot::capture(fixture.notes.path().canonicalize().unwrap()).unwrap();
    let selection = Selection {
        agent: Agent::Codex,
        scope: Scope::Project,
        root: root.path,
        identity: root.identity,
    };
    let grant = selection.id().unwrap();
    let setup = |action| call(fixture.data.path(), &Request::AgentSetup { action }).unwrap();
    setup(Action::Install {
        selection: selection.clone(),
        executable: std::env::current_exe().unwrap(),
        client_version: "codex 0.100.0".into(),
    });
    let mut client = Client::grant(fixture.data.path(), &grant);
    client.ready();
    assert_eq!(
        client.tool("search", json!({"query":"Durable"}))["result"]["isError"],
        true
    );
    setup(Action::SetMcp {
        selection: selection.clone(),
        sources: vec![fixture.id.clone()],
        enabled: true,
    });
    let sources = data(&client.tool("list_sources", json!({})));
    assert_eq!(sources.as_array().unwrap().len(), 1);
    assert_eq!(sources[0]["id"], fixture.id);
    let hits = data(&client.tool("search", json!({"query":"Durable"})));
    assert!(!hits.as_array().unwrap().is_empty());
    assert_eq!(
        client.tool(
            "read_document",
            json!({"source_id":fixture.hidden,"path":"secret.md"})
        )["result"]["isError"],
        true
    );
    setup(Action::SetMcp {
        selection: selection.clone(),
        sources: vec![],
        enabled: false,
    });
    assert_eq!(
        client.tool(
            "read_document",
            json!({"source_id":fixture.id,"path":"plan.md"})
        )["result"]["isError"],
        true
    );
    setup(Action::SetMcp {
        selection,
        sources: vec![fixture.id.clone()],
        enabled: true,
    });
    call(
        fixture.data.path(),
        &Request::SetSourceEnabled {
            source_id: fixture.id.clone(),
            enabled: false,
        },
    )
    .unwrap();
    assert_eq!(
        client.tool("search", json!({"query":"Durable"}))["result"]["isError"],
        true
    );
}

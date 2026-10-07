//! Disposable synthetic-core benchmark. No model, user library, or service config.
//! Run the release example; --help documents bounded link-operation subprocesses.
use mdreader_core::{ReaderStore, SourceRoot};
use serde_json::{json, Value};
use std::error::Error;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
fn duration_ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}
fn summary(samples: &[f64]) -> Value {
    if samples.is_empty() {
        return json!({"samples":0,"p50_ms":null,"p95_ms":null,"min_ms":null,"max_ms":null});
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let percentile = |p: f64| sorted[((sorted.len() as f64 * p).ceil() as usize).saturating_sub(1)];
    json!({"samples":samples.len(),"p50_ms":percentile(0.50),"p95_ms":percentile(0.95),"min_ms":sorted[0],"max_ms":sorted[sorted.len()-1]})
}
fn directory_sizes(directory: &Path) -> Result<Value> {
    let mut values = serde_json::Map::new();
    let mut total = 0u64;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_file() {
            let bytes = metadata.len();
            total += bytes;
            values.insert(
                entry.file_name().to_string_lossy().into_owned(),
                json!(bytes),
            );
        }
    }
    values.insert("total_bytes".into(), json!(total));
    Ok(Value::Object(values))
}
fn plans(data: &Path, source: &str) -> Result<Value> {
    let database = rusqlite::Connection::open_with_flags(
        data.join("index.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let queries=[
        ("resolve_source_documents","SELECT id,path,revision,title FROM documents WHERE source_id=?1 ORDER BY path"),
        ("related_source_links","SELECT d.path,l.target FROM links l JOIN documents d ON d.id=l.document_id WHERE d.source_id=?1 AND l.kind<>'image' ORDER BY d.path,l.line LIMIT 100001"),
        ("linked_to_source_links","SELECT d.id,d.path,l.target FROM links l JOIN documents d ON d.id=l.document_id WHERE d.source_id=?1 AND l.kind!='image' LIMIT 100001"),
    ];
    let mut result = serde_json::Map::new();
    for (name, sql) in queries {
        let rows = database
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?
            .query_map([source], |row| row.get::<_, String>(3))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        result.insert(name.into(), json!(rows));
    }
    let version: i64 = database.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version >= 4 {
        let indexed_queries = [
            ("v4_resolve_exact", "SELECT id,path,revision FROM documents WHERE source_id=?1 AND path IN('note000001.md','note000001.md.md') ORDER BY path"),
            ("v4_alias_lookup", "SELECT document_id FROM document_aliases WHERE source_id=?1 AND kind='stem' AND alias='missing'"),
            ("v4_incoming_edges", "SELECT origin_id FROM resolved_links WHERE target_id=(SELECT id FROM documents WHERE source_id=?1 AND path='note000001.md')"),
            ("v4_related_outgoing", "SELECT target_id,target_line FROM resolved_links WHERE source_id=?1 AND origin_id=(SELECT id FROM documents WHERE source_id=?1 AND path='note000001.md')"),
            ("v4_related_incoming", "SELECT origin_id FROM resolved_links WHERE source_id=?1 AND target_id=(SELECT id FROM documents WHERE source_id=?1 AND path='note000001.md')"),
            ("v4_linked_to_full_search", "SELECT c.id FROM chunks c JOIN documents d ON d.id=c.document_id WHERE d.source_id IN(?1) AND d.id IN(SELECT r.origin_id FROM resolved_links r WHERE r.target_id IN(SELECT value FROM json_each('[2]'))) ORDER BY d.modified_ms DESC,d.path,c.id LIMIT 20"),
            ("path_full_search", "SELECT c.id FROM chunks c JOIN documents d ON d.id=c.document_id WHERE d.source_id IN(?1) AND d.path GLOB '*note000000.md*' ORDER BY d.modified_ms DESC,d.path,c.id LIMIT 20"),
        ];
        for (name, sql) in indexed_queries {
            let rows = database
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?
                .query_map([source], |row| row.get::<_, String>(3))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            result.insert(name.into(), json!(rows));
        }
    }
    Ok(Value::Object(result))
}
fn worker(args: &[String]) -> Result<()> {
    let store = ReaderStore::open(Path::new(&args[0]))?;
    let source = &args[1];
    let operation = &args[2];
    let samples: usize = args[3].parse()?;
    for sample in 0..samples {
        let start = Instant::now();
        let hits = match operation.as_str() {
            "resolve_exact" => store
                .resolve_reference(source, "note000000.md", "note000001.md#details")?
                .len(),
            "resolve_missing" => store
                .resolve_reference(source, "note000000.md", "missing.md")?
                .len(),
            "related" => store.related(source, "note000001.md", 20)?.len(),
            "linked_to" => store
                .search("linked-to:note000001.md", std::slice::from_ref(source), 20)?
                .len(),
            _ => return Err("unknown operation".into()),
        };
        let elapsed_ms = duration_ms(start);
        if operation == "resolve_exact" {
            assert_eq!(hits, 1);
        }
        if operation == "resolve_missing" {
            assert_eq!(hits, 0);
        }
        if operation == "related" {
            assert_eq!(hits, 2);
        }
        if operation == "linked_to" {
            assert_eq!(hits, 2, "two chunks in the one incoming document");
        }
        println!(
            "{}",
            json!({"sample":sample,"elapsed_ms":elapsed_ms,"hits":hits})
        );
        std::io::stdout().flush()?;
    }
    Ok(())
}
fn bounded_operation(
    data: &Path,
    source: &str,
    operation: &str,
    samples: usize,
    seconds: u64,
) -> Result<Value> {
    let start = Instant::now();
    let mut child = Command::new(std::env::current_exe()?)
        .arg("--operation")
        .arg(data)
        .arg(source)
        .arg(operation)
        .arg(samples.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() >= Duration::from_secs(seconds) {
            timed_out = true;
            child.kill()?;
            break child.wait()?;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let wall_ms = duration_ms(start);
    let mut text = String::new();
    child.stdout.take().unwrap().read_to_string(&mut text)?;
    let mut stderr = String::new();
    child.stderr.take().unwrap().read_to_string(&mut stderr)?;
    if !status.success() && !timed_out {
        return Err(format!("operation {operation} failed: {stderr}").into());
    }
    let records = text
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let times = records
        .iter()
        .map(|v| v["elapsed_ms"].as_f64().unwrap())
        .collect::<Vec<_>>();
    Ok(
        json!({"operation":operation,"requested_samples":samples,"timed_out":timed_out,"budget_seconds":seconds,"worker_wall_ms":wall_ms,"timing":summary(&times),"records":records,"note":"Samples are sequential after a fresh core reopen. Timeout wall includes reopen and previous completed samples; it is not an exact single-call lower bound."}),
    )
}
fn corpus(count: usize, samples: usize, link_budget: u64) -> Result<Value> {
    eprintln!("synthetic corpus: {count} documents");
    let temporary = tempfile::tempdir()?;
    let notes = temporary.path().join("notes");
    let data = temporary.path().join("data");
    fs::create_dir(&notes)?;
    let generation = Instant::now();
    let mut source_bytes = 0u64;
    for index in 0..count {
        let body=format!("---\ntags: [benchmark, topic{:03}]\n---\n# Document {index:06}\n\nMDReader durable local reading search evidence. topic{:03} sentinel{index:06}.\nOffline synchronization preserves the original Markdown and scoped revision evidence.\n\n## Details\n\nA deterministic synthetic document measures lexical lookup and link navigation.\n[Next](note{:06}.md#details)\n",index%97,index%97,(index+1)%count);
        source_bytes += body.len() as u64;
        fs::write(notes.join(format!("note{index:06}.md")), body)?;
    }
    let generation_ms = duration_ms(generation);
    let mut store = ReaderStore::open(&data)?;
    let source = store.add_local_source(
        "Synthetic benchmark",
        SourceRoot::capture(notes.canonicalize()?)?,
    )?;
    let first = Instant::now();
    let indexed = store.index_source(&source.id)?;
    let cold_index_ms = duration_ms(first);
    assert_eq!(indexed, count);
    let status = store.index_status(std::slice::from_ref(&source.id))?;
    assert_eq!(status.documents, count as u64);
    assert_eq!(status.embedded, 0);
    let unchanged = Instant::now();
    store.index_source(&source.id)?;
    let unchanged_index_ms = duration_ms(unchanged);
    let changed_path = notes.join("note000000.md");
    let mut changed = fs::read_to_string(&changed_path)?;
    let appended = "\nA single changed document retains the original title and links.\n";
    changed.push_str(appended);
    source_bytes += appended.len() as u64;
    fs::write(changed_path, changed)?;
    let one_changed = Instant::now();
    store.index_source(&source.id)?;
    let one_changed_index_ms = duration_ms(one_changed);
    let queries = [
        "reading".to_owned(),
        "topic003".to_owned(),
        format!("sentinel{:06}", count / 2),
        "reading tag:topic003".to_owned(),
        "path:note000000.md".to_owned(),
    ];
    let mut lexical = Vec::new();
    for query in queries {
        for _ in 0..5 {
            let hits = store.search(&query, std::slice::from_ref(&source.id), 20)?;
            assert!(!hits.is_empty(), "query must match: {query}");
        }
        let mut times = Vec::new();
        let mut returned = Vec::new();
        for _ in 0..samples {
            let start = Instant::now();
            let hits = store.search(&query, std::slice::from_ref(&source.id), 20)?;
            times.push(duration_ms(start));
            assert!(!hits.is_empty());
            returned.push(hits.len());
        }
        lexical.push(json!({"query":query,"timing":summary(&times),"samples_ms":times,"returned_hits":returned}));
    }
    let live_sizes = directory_sizes(&data)?;
    drop(store);
    let closed_sizes = directory_sizes(&data)?;
    let query_plans = plans(&data, &source.id)?;
    let mut operations = Vec::new();
    for operation in ["resolve_exact", "resolve_missing", "linked_to", "related"] {
        let requested = if operation.starts_with("resolve_") {
            20
        } else {
            3
        };
        eprintln!("  {count}: {operation}, budget {link_budget}s");
        operations.push(bounded_operation(
            &data,
            &source.id,
            operation,
            requested,
            link_budget,
        )?);
    }
    Ok(
        json!({"documents":count,"source_files":count,"source_bytes":source_bytes,"links_expected":count,"generation_ms":generation_ms,"index_status":status,"cold_empty_index_ms":cold_index_ms,"unchanged_index_ms":unchanged_index_ms,"one_changed_index_ms":one_changed_index_ms,"lexical":lexical,"operations":operations,"sqlite_live_bytes":live_sizes,"sqlite_closed_bytes":closed_sizes,"query_plans":query_plans}),
    )
}
fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.first().map(String::as_str) == Some("--operation") {
        return worker(&args[1..]);
    }
    if args.iter().any(|arg| arg == "--help") {
        println!("synthetic_perf [--sizes 1000,10000,50000] [--samples 50] [--link-budget-seconds 15] [--json-out PATH]\nAll corpora/database files are temporary and deleted. No models or private data. Expensive read-only link operations run in bounded subprocesses.");
        return Ok(());
    }
    let mut sizes = vec![1000, 10000, 50000];
    let mut samples = 50;
    let mut link_budget = 15;
    let mut output: Option<PathBuf> = None;
    let mut arguments = args.iter();
    while let Some(option) = arguments.next() {
        let value = arguments.next().ok_or("missing option value")?;
        match option.as_str() {
            "--sizes" => {
                sizes = value
                    .split(',')
                    .map(str::parse)
                    .collect::<std::result::Result<_, _>>()?
            }
            "--samples" => samples = value.parse()?,
            "--link-budget-seconds" => link_budget = value.parse()?,
            "--json-out" => output = Some(value.into()),
            _ => return Err("unknown option".into()),
        }
    }
    if sizes.iter().any(|n| *n < 3 || *n > 50000)
        || samples == 0
        || samples > 1000
        || link_budget == 0
        || link_budget > 60
    {
        return Err("benchmark bounds exceeded".into());
    }
    let mut results = Vec::new();
    for size in sizes {
        results.push(corpus(size, samples, link_budget)?);
    }
    let report = json!({"benchmark":"native-core-synthetic-v1","profile":if cfg!(debug_assertions) {"debug"} else {"release"},"store_source_blake3":blake3::hash(include_str!("../src/store.rs").as_bytes()).to_hex().to_string(),"query_source_blake3":blake3::hash(include_str!("../src/query.rs").as_bytes()).to_hex().to_string(),"os":std::env::consts::OS,"arch":std::env::consts::ARCH,"model_enabled":false,"samples_per_lexical_query":samples,"warmups_per_lexical_query":5,"percentiles":"nearest-rank","results":results});
    let bytes = serde_json::to_vec_pretty(&report)?;
    if let Some(output) = output {
        fs::write(output, bytes)?;
    } else {
        std::io::stdout().write_all(&bytes)?;
        println!();
    }
    Ok(())
}

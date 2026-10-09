// Prevents an extra console window on Windows in release builds. DO NOT REMOVE.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// Retained in every shipping executable so post-packaging verification can
// bind real ELF/PE/Mach-O bytes to the reviewed source and product. NUL
// delimiters avoid accidental matches in ordinary UI strings.
#[used]
static MDREADER_RELEASE_IDENTITY: &str = concat!(
    "MDREADER_RELEASE_IDENTITY_V1\0",
    "productName=",
    env!("MDREADER_NATIVE_PRODUCT_NAME"),
    "\0bundleIdentifier=",
    env!("MDREADER_NATIVE_APP_ID"),
    "\0",
    "sourceCommit=",
    env!("MDREADER_RELEASE_SOURCE_COMMIT"),
    "\0",
);

#[cfg(not(mobile))]
fn main() {
    std::hint::black_box(MDREADER_RELEASE_IDENTITY);
    let mut args = std::env::args_os().skip(1);
    let first = args.next();
    if first
        .as_ref()
        .is_some_and(|arg| arg == "--mdreader-legacy-export")
    {
        use std::io::Write;
        let result = mdreader_legacy_export::cli::run(args).and_then(|bytes| {
            std::io::stdout()
                .lock()
                .write_all(&bytes)
                .map_err(mdreader_legacy_export::Error::from)
        });
        if result.is_err() {
            eprintln!(
                "Offline migration failed; originals retained. {}",
                mdreader_legacy_export::cli::USAGE
            );
            std::process::exit(1);
        }
        return;
    }
    #[cfg(any(unix, windows))]
    if first
        .as_ref()
        .is_some_and(|arg| arg == "--reader-selection")
    {
        let Some(journal) = args.next().filter(|_| args.next().is_none()) else {
            eprintln!("Profile selection unavailable; originals retained");
            std::process::exit(2);
        };
        if mdreader_app_lib::run_profile_selection(std::path::PathBuf::from(journal)).is_err() {
            eprintln!("Profile selection unavailable; originals retained");
            std::process::exit(1);
        }
        return;
    }
    #[cfg(unix)]
    if first
        .as_ref()
        .is_some_and(|arg| arg == "--reader-return-controller")
    {
        let Some(candidate) = args.next().filter(|_| args.next().is_none()) else {
            eprintln!("Return selection unavailable; libraries retained");
            std::process::exit(2);
        };
        if mdreader_app_lib::run_return_controller(std::path::PathBuf::from(candidate)).is_err() {
            eprintln!("Return selection unavailable; libraries retained");
            std::process::exit(1);
        }
        return;
    }
    if first.as_ref().is_some_and(|arg| arg == "--reader-profile") {
        let Some(profile) = args.next().filter(|_| args.next().is_none()) else {
            eprintln!("usage: MDReader --reader-profile ABS_PREPARED_PROFILE");
            std::process::exit(2)
        };
        if mdreader_app_lib::run_prepared_profile(std::path::PathBuf::from(profile)).is_err() {
            eprintln!("Prepared reader profile unavailable; originals retained");
            std::process::exit(1)
        }
        return;
    }
    if first
        .as_ref()
        .is_some_and(|arg| arg == "--mdreader-capture")
    {
        let result = mdreader_capture::cli::run(args, std::io::stdin().lock());
        println!("{{}}");
        if let Err(error) = result {
            eprintln!("MDReader capture failed: {error}");
            std::process::exit(1);
        }
        return;
    }
    if first.as_ref().is_some_and(|arg| arg == "--mdreader-mcp") {
        let config = mdreader_mcp::Config::from_args(args).unwrap_or_else(|message| {
            eprintln!("{message}");
            std::process::exit(2)
        });
        let executable = std::env::current_exe().unwrap_or_else(|_| {
            eprintln!("Reader executable unavailable");
            std::process::exit(1)
        });
        if mdreader_service::ensure_running(
            config.data_dir(),
            &executable,
            &["--mdreader-service".into()],
        )
        .is_err()
        {
            eprintln!("Reader service unavailable");
            std::process::exit(1);
        }
        if mdreader_mcp::serve(config, std::io::stdin().lock(), std::io::stdout().lock()).is_err() {
            eprintln!("MCP transport closed");
            std::process::exit(1);
        }
        return;
    }
    if first.is_some_and(|arg| arg == "--mdreader-service") {
        let first = args.next();
        let disable_model = first.as_ref().is_some_and(|arg| arg == "--no-model");
        let directory = if disable_model { args.next() } else { first };
        let Some(directory) = directory.filter(|_| args.next().is_none()) else {
            eprintln!("usage: MDReader --mdreader-service [--no-model] DATA_DIRECTORY");
            std::process::exit(2);
        };
        if let Err(error) = mdreader_app_lib::serve_reader_service(
            std::path::Path::new(&directory),
            mdreader_service::ServiceOptions { disable_model },
        ) {
            eprintln!("Reader service failed: {error}");
            std::process::exit(1);
        }
        return;
    }
    mdreader_app_lib::run()
}

#[cfg(mobile)]
fn main() {
    std::hint::black_box(MDREADER_RELEASE_IDENTITY);
    mdreader_app_lib::run()
}

//! Standalone bounded capture entrypoint; never emits an agent decision.
fn main() -> std::process::ExitCode {
    let result = mdreader_capture::cli::run(std::env::args_os().skip(1), std::io::stdin().lock());
    println!("{{}}");
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("MDReader capture failed: {error}");
            std::process::ExitCode::from(1)
        }
    }
}

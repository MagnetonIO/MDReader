mod cli;
use std::io::Write;
fn main() {
    let result = cli::run(std::env::args_os().skip(1)).and_then(|bytes| {
        std::io::stdout()
            .lock()
            .write_all(&bytes)
            .map_err(mdreader_legacy_export::Error::from)
    });
    if let Err(error) = result {
        eprintln!("{error}; {}", cli::USAGE);
        std::process::exit(1)
    }
}

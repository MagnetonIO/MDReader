fn main() {
    let config = match mdreader_mcp::Config::from_args(std::env::args_os().skip(1)) {
        Ok(config) => config,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    if mdreader_mcp::serve(config, stdin.lock(), stdout.lock()).is_err() {
        eprintln!("MCP transport closed");
        std::process::exit(1);
    }
}

fn main() {
    let mut args = std::env::args_os().skip(1);
    let first = args.next();
    let disable_model = first.as_ref().is_some_and(|arg| arg == "--no-model");
    let directory = if disable_model { args.next() } else { first };
    let Some(directory) = directory.filter(|_| args.next().is_none()) else {
        eprintln!("usage: mdreader-service [--no-model] DATA_DIRECTORY");
        std::process::exit(2);
    };
    if let Err(error) = mdreader_service::serve_with_options(
        std::path::Path::new(&directory),
        mdreader_service::ServiceOptions { disable_model },
    ) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

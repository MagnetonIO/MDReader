//! Synthetic native lifetime participant for Windows migration tests only.
//! CI compiles this outside the repository as MDReader.exe. No files, account
//! state, GUI or service are opened; stdin requests an ordinary successful exit.
use std::io::{self, BufRead, Write};
fn main() -> io::Result<()> {
    println!("ready");
    io::stdout().flush()?;
    let mut request = String::new();
    io::stdin().lock().read_line(&mut request)?;
    if request != "quit\n" {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "expected quit"));
    }
    Ok(())
}

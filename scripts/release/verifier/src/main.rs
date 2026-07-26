use std::{env, fs, process};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use minisign_verify::{PublicKey, Signature};

fn decode_document(value: &str, label: &str) -> Result<String, String> {
    let bytes = STANDARD
        .decode(value)
        .map_err(|_| format!("{label} is not valid base64"))?;
    String::from_utf8(bytes).map_err(|_| format!("{label} is not valid UTF-8"))
}

fn verify() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let artifact_path = args
        .next()
        .ok_or_else(|| "missing updater artifact path".to_string())?;
    let signature_value = args
        .next()
        .ok_or_else(|| "missing updater signature".to_string())?;
    let public_key_value = args
        .next()
        .ok_or_else(|| "missing updater public key".to_string())?;
    if args.next().is_some() {
        return Err("unexpected updater verifier argument".to_string());
    }

    let artifact = fs::read(&artifact_path)
        .map_err(|error| format!("cannot read updater artifact: {error}"))?;
    let signature_document = decode_document(&signature_value, "updater signature")?;
    let public_key_document = decode_document(&public_key_value, "updater public key")?;
    let signature = Signature::decode(&signature_document)
        .map_err(|error| format!("invalid updater signature document: {error}"))?;
    let public_key = PublicKey::decode(&public_key_document)
        .map_err(|error| format!("invalid updater public key document: {error}"))?;
    public_key
        .verify(&artifact, &signature, true)
        .map_err(|error| format!("updater signature verification failed: {error}"))
}

fn main() {
    if let Err(error) = verify() {
        eprintln!("{error}");
        process::exit(1);
    }
}

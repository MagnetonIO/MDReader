//! Explicit source identity only; a malformed/present handoff never falls back.
use std::{ffi::OsStr, fs, io::Read, path::Path};

pub const UNSET: &str = "0000000000000000000000000000000000000000";

fn sha(value: &str) -> Result<&str, &'static str> {
    if value.len() != 40
        || value == UNSET
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("release source identity must be a lowercase nonzero 40-character Git SHA");
    }
    Ok(value)
}

fn handoff(path: &Path) -> Result<Option<String>, &'static str> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("cannot inspect release source-identity handoff"),
    };
    if !metadata.file_type().is_file() || metadata.len() != 41 {
        return Err("release source-identity handoff must be a regular file containing SHA plus one newline");
    }
    let file = fs::File::open(path).map_err(|_| "cannot open release source-identity handoff")?;
    let opened = file
        .metadata()
        .map_err(|_| "cannot inspect opened release source-identity handoff")?;
    if !opened.is_file() || opened.len() != 41 {
        return Err("release source-identity handoff changed while opening");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.dev() != opened.dev() || metadata.ino() != opened.ino() {
            return Err("release source-identity handoff was replaced while opening");
        }
    }
    let mut bytes = Vec::with_capacity(42);
    file.take(42)
        .read_to_end(&mut bytes)
        .map_err(|_| "cannot read release source-identity handoff")?;
    if bytes.len() != 41 || bytes[40] != b'\n' {
        return Err(
            "release source-identity handoff must contain exactly one newline-terminated SHA",
        );
    }
    let value = std::str::from_utf8(&bytes[..40])
        .map_err(|_| "release source-identity handoff is not UTF-8")?;
    Ok(Some(sha(value)?.to_owned()))
}

/// Absence alone permits the legacy zero/unbound development stamp. Explicit zero,
/// non-Unicode environment values, malformed files and conflicting sources fail.
pub fn resolve(environment: Option<&OsStr>, path: &Path) -> Result<String, &'static str> {
    let environment = environment
        .map(|value| {
            value
                .to_str()
                .ok_or("release source environment is not Unicode")
                .and_then(sha)
        })
        .transpose()?;
    let file = handoff(path)?;
    if environment
        .zip(file.as_deref())
        .is_some_and(|(env, file)| env != file)
    {
        return Err("release source environment and handoff identities do not match");
    }
    Ok(environment.or(file.as_deref()).unwrap_or(UNSET).to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
    struct Temporary(PathBuf);
    impl Temporary {
        fn new() -> Self {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "mdreader-release-identity-{}-{stamp}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn file(&self) -> PathBuf {
            self.0.join(".release-source-commit")
        }
    }
    impl Drop for Temporary {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    #[test]
    fn optional_development_identity_and_explicit_consistent_sources() {
        let temp = Temporary::new();
        assert_eq!(resolve(None, &temp.file()).unwrap(), UNSET);
        assert_eq!(
            resolve(Some(OsStr::new(COMMIT)), &temp.file()).unwrap(),
            COMMIT
        );
        fs::write(temp.file(), format!("{COMMIT}\n")).unwrap();
        assert_eq!(resolve(None, &temp.file()).unwrap(), COMMIT);
        assert_eq!(
            resolve(Some(OsStr::new(COMMIT)), &temp.file()).unwrap(),
            COMMIT
        );
        assert!(resolve(
            Some(OsStr::new("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")),
            &temp.file()
        )
        .is_err());
    }
    #[test]
    fn present_malformed_identity_never_falls_back_to_valid_other_source() {
        let temp = Temporary::new();
        for bad in [
            String::new(),
            UNSET.into(),
            COMMIT.to_uppercase(),
            format!("{COMMIT}\n"),
        ] {
            assert!(resolve(Some(OsStr::new(&bad)), &temp.file()).is_err());
        }
        for bad in [
            String::new(),
            COMMIT.into(),
            format!("{COMMIT}\r\n"),
            format!("{COMMIT}\n\n"),
            format!("{UNSET}\n"),
            "x".repeat(1024),
        ] {
            fs::write(temp.file(), bad).unwrap();
            assert!(resolve(Some(OsStr::new(COMMIT)), &temp.file()).is_err());
        }
        fs::remove_file(temp.file()).unwrap();
        fs::create_dir(temp.file()).unwrap();
        assert!(resolve(None, &temp.file()).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn symlink_and_non_unicode_environment_are_rejected() {
        use std::os::unix::{ffi::OsStrExt, fs::symlink};
        let temp = Temporary::new();
        fs::write(temp.0.join("other"), format!("{COMMIT}\n")).unwrap();
        symlink(temp.0.join("other"), temp.file()).unwrap();
        assert!(resolve(None, &temp.file()).is_err());
        assert!(resolve(Some(OsStr::from_bytes(&[255])), &temp.0.join("absent")).is_err());
    }
}

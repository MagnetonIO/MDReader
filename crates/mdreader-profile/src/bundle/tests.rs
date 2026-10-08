use super::*;
use crate::{Disposition, InventorySelection, RootLocator};
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::PathBuf,
};

struct Owner {
    root: RootBinding,
    fail_after_preimages: bool,
    issued: bool,
}
impl FreezeOwner for Owner {
    fn with_frozen<T>(
        &mut self,
        expected: &RootBinding,
        action: impl FnOnce(&mut dyn FrozenProfile) -> Result<T>,
    ) -> Result<T> {
        if expected != &self.root {
            return Err(Error::Conflict);
        }
        action(self)
    }
}
impl FrozenProfile for Owner {
    fn binding(&self) -> &RootBinding {
        &self.root
    }
    fn revalidate(&mut self) -> Result<()> {
        if self.fail_after_preimages && self.issued {
            return Err(Error::NotQuiescent);
        }
        let metadata = fs::metadata(self.root.locator.restore()?).map_err(|_| Error::Storage)?;
        if (metadata.dev(), metadata.ino()) != (self.root.device, self.root.inode) {
            return Err(Error::Conflict);
        }
        Ok(())
    }
    fn preflush_json(&mut self) -> Result<Vec<PreflushArtifact>> {
        self.issued = true;
        Ok(vec![
            PreflushArtifact {
                kind: PreflushKind::Session,
                bytes: None,
            },
            PreflushArtifact {
                kind: PreflushKind::Settings,
                bytes: Some(Vec::new()),
            },
        ])
    }
    fn with_database(
        &mut self,
        _: &Path,
        _: &mut dyn FnMut(&rusqlite::Connection) -> Result<()>,
    ) -> Result<()> {
        Err(Error::Invalid)
    }
}

#[test]
fn outer_bundle_preserves_absent_vs_empty_and_never_publishes_unverified_or_replaces() {
    let temp = tempfile::Builder::new()
        .tempdir_in(std::env::temp_dir().canonicalize().unwrap())
        .unwrap();
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let source = temp.path().join("original");
    fs::create_dir(&source).unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o700)).unwrap();
    let original = b"{\"settings\":{\"theme\":\"light\"},\"unknown\":900719925474099312345}";
    fs::write(source.join("settings.json"), original).unwrap();
    let metadata = fs::metadata(&source).unwrap();
    let mut owner = Owner {
        root: RootBinding {
            locator: RootLocator::capture(&source).unwrap(),
            device: metadata.dev(),
            inode: metadata.ino(),
        },
        fail_after_preimages: true,
        issued: false,
    };
    let choices = [InventorySelection {
        relative: PathBuf::from("settings.json"),
        disposition: Disposition::Raw,
    }];
    let destination = temp.path().join("preserved");
    let limits = Limits::default();
    assert!(matches!(
        stage_bundle(
            &mut owner,
            Inventory::capture(&source, &choices, &limits).unwrap(),
            &destination,
            &limits
        ),
        Err(Error::NotQuiescent)
    ));
    assert!(!destination.exists());
    assert_eq!(fs::read(source.join("settings.json")).unwrap(), original);
    owner.fail_after_preimages = false;
    let verified = stage_bundle(
        &mut owner,
        Inventory::capture(&source, &choices, &limits).unwrap(),
        &destination,
        &limits,
    )
    .unwrap();
    assert!(!verified.manifest().preflush[0].present);
    assert!(verified.manifest().preflush[1].present);
    assert_eq!(verified.manifest().preflush[1].bytes, 0);
    let expected = verified.identity().clone();
    verify_bundle(&destination, &expected, &limits).unwrap();
    assert!(matches!(
        stage_bundle(
            &mut owner,
            Inventory::capture(&source, &choices, &limits).unwrap(),
            &destination,
            &limits
        ),
        Err(Error::Conflict)
    ));
    verify_bundle(&destination, &expected, &limits).unwrap();
    fs::write(destination.join("settings.preflush"), b"tampered").unwrap();
    assert!(matches!(
        verify_bundle(&destination, &expected, &limits),
        Err(Error::Integrity)
    ));
    assert_eq!(fs::read(source.join("settings.json")).unwrap(), original);
}

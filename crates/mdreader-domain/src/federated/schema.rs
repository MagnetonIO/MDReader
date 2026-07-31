//! The federated `Document` schema: one concept shared by every Library source.
//!
//! # Why one entity
//!
//! The claim this schema serves is that ONE semantic concept can have its
//! attributes answered by genuinely different physical sources. Splitting the
//! concept into `Document` plus a per-source `Observation` entity would make
//! that claim trivially true and semantically empty — the sources would each
//! own their own object and nothing would be federated. So there is exactly
//! one entity, and the per-source observations live at the *instance* level
//! (see [`Document::observations`](crate::federated::Document::observations)),
//! where they belong: they are rows, not concepts.
//!
//! # Why these attributes
//!
//! Four of them are the claim:
//!
//! ```text
//! Document.title       <- YAML frontmatter | first H1
//! Document.modifiedAt  <- filesystem mtime | Git commit timestamp
//! Document.author      <- YAML frontmatter | Git commit author
//! Document.path        <- filesystem
//! ```
//!
//! `size` and `body` are required by the local adapters. Provider identity,
//! media type, revision, and checksum are required by Drive and Dropbox.
//! Schema mappings are **total** over their source attributes
//! ([`Mapping::validate`](catdb_sdk::core::Mapping::validate)); leaving any of these
//! fields out would silently fork the semantic concept or invalidate a source
//! mapping. The honest representation is one widened `Document`.
//!
//! # Why `path` is the only non-nullable attribute
//!
//! Every other attribute may genuinely be unobservable for a given document —
//! a file with no YAML `author:` and no commit has no author, and the honest
//! record of that is an absent value. `path` is different in kind: it is the
//! semantic record key every adapter emits for every row it produces: a local
//! path, Drive file id, or Dropbox path. A `Document` with no key is an
//! unidentifiable observation. The assembler enforces it structurally —
//! [`Document::path`] is a `String`, not an `Option<String>`, and a row without
//! one is a typed refusal.
//!
//! [`Document::path`]: crate::federated::Document::path

use catdb_sdk::core::{builtin::standard_typeside, Schema};

/// The federated schema's name.
pub const SCHEMA: &str = "document";

/// The one entity the schema declares.
pub const ENTITY: &str = "Document";

/// The `Document` attributes, with the typeside type each holds and whether it
/// may be absent. In declaration order; the schema stores them by name.
///
/// Public because it is the single place the concept is written down: the
/// mappings, the precedence declaration and the tests all check themselves
/// against this list rather than against a second copy of it.
pub const ATTRIBUTES: &[(&str, &str, bool)] = &[
    // (name, typeside type, nullable)
    ("providerId", "Text", true),
    ("path", "Text", false),
    ("title", "Text", true),
    ("author", "Text", true),
    ("mediaType", "Text", true),
    ("modifiedAt", "Timestamp", true),
    ("size", "Int", true),
    ("revision", "Text", true),
    ("checksum", "Text", true),
    ("body", "Text", true),
];

/// The federated `Document` schema, over the kernel's standard typeside.
///
/// Kernel-validated exactly like the builtin overlays: see
/// [`crate::validate_all`], which records it alongside them.
pub fn document_schema() -> Schema {
    let mut schema = Schema::new(SCHEMA, standard_typeside());
    schema.add_entity(ENTITY);
    for (name, ty, nullable) in ATTRIBUTES {
        schema.add_attribute(*name, ENTITY, *ty, *nullable);
    }
    schema
}

/// The qualified name of the concept, as a query writes it.
pub fn qualified_entity() -> String {
    format!("{SCHEMA}.{ENTITY}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_schema_is_exactly_the_declared_attribute_list() {
        let schema = document_schema();
        assert_eq!(schema.validate(), Ok(()));
        assert_eq!(schema.entities.len(), 1);
        assert_eq!(schema.attributes.len(), ATTRIBUTES.len());
        for (name, ty, nullable) in ATTRIBUTES {
            let attr = &schema.attributes[&(*name).into()];
            assert_eq!(attr.ty.as_str(), *ty, "`{name}` type");
            assert_eq!(attr.nullable, *nullable, "`{name}` nullability");
            assert_eq!(attr.dom.as_str(), ENTITY);
        }
    }

    /// The key is the one thing that is never absent.
    #[test]
    fn path_is_the_only_attribute_that_may_not_be_absent() {
        let required: Vec<&str> = ATTRIBUTES
            .iter()
            .filter(|(_, _, nullable)| !nullable)
            .map(|(name, _, _)| *name)
            .collect();
        assert_eq!(required, ["path"]);
    }
}

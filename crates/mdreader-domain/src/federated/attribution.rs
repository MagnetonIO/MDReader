//! Field-level source attribution: which source answered, which were silent,
//! and which never got asked.
//!
//! # Why attribution is the value, not a log line
//!
//! A `Document` whose `author` is `"M. Long"` is not the interesting object. A
//! `Document` whose `author` is `"M. Long"` **because the frontmatter source
//! observed it and the git source was outranked** is. Once attributes come
//! from different physical places, "where did this come from?" is part of the
//! answer, and an answer that drops it is not smaller — it is unverifiable.
//!
//! So there is no plain `Option<String>` anywhere in this module. Every field
//! of an assembled document is a [`FieldAttribution`], which carries what each
//! declared provider had to say, in declared precedence order. Reading the
//! value is [`FieldAttribution::value`]; reading who answered is
//! [`FieldAttribution::answered_by`]; both are derived from the same record, so
//! they cannot drift apart, and neither can be produced without the other.
//!
//! # Four verdicts, because there are four cases
//!
//! [`ProviderVerdict`] separates "read the document and saw no author" from
//! "never read this document" from "was never asked at all". Collapsing them
//! into `None` would make an unavailable git repository look exactly like a
//! document nobody wrote — which is the single mistake this whole design is
//! shaped to prevent.
//!
//! # Nothing is merged
//!
//! Resolution *selects*; it never combines. The rows the adapters produced are
//! kept verbatim on [`Document::observations`], and the
//! [`IdentityClaim`]s that say they denote one document are kept on
//! [`Document::claims`]. Both records of a claim remain, exactly as
//! [`IdentityRule`](catdb_core::IdentityRule) requires.

use std::collections::BTreeMap;

use catdb_connectors::{SourceId, SourceRow};
use catdb_core::{AttrName, IdentityClaim};
use serde::Serialize;

use super::precedence::Provider;

/// What one declared provider had to say about one field of one document.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum ProviderVerdict {
    /// The provider's source observed a value, given here in the `Document`'s
    /// representation (i.e. after the mapping's declared transformation chain).
    Observed(serde_json::Value),
    /// The source read this document and did not observe this attribute for
    /// it. The honest "there is nothing here" — a YAML block with no
    /// `author:`, a commit-less file with no commit author.
    ObservedNothing,
    /// The source ran, but this document is not among the objects it
    /// describes. The frontmatter source says this about a PNG; git says it
    /// about an untracked file.
    DidNotSeeDocument,
    /// The source was never consulted: it is not part of this federation, or
    /// it declared itself unavailable here.
    NotAsked {
        /// Why, in the source's own words where it has any.
        reason: String,
    },
}

impl ProviderVerdict {
    /// The observed value, if this provider observed one.
    pub fn observed(&self) -> Option<&serde_json::Value> {
        match self {
            ProviderVerdict::Observed(value) => Some(value),
            _ => None,
        }
    }

    /// A stable lowercase word for the verdict, for rendering and assertions.
    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderVerdict::Observed(_) => "observed",
            ProviderVerdict::ObservedNothing => "observed-nothing",
            ProviderVerdict::DidNotSeeDocument => "did-not-see-document",
            ProviderVerdict::NotAsked { .. } => "not-asked",
        }
    }
}

/// One `Document` field, with the whole record of how it got its value.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FieldAttribution {
    /// The `Document` attribute.
    pub field: AttrName,
    /// Every declared provider, in declared precedence order, with its
    /// verdict. The winner is the first [`ProviderVerdict::Observed`]; a field
    /// with no `Observed` verdict is absent.
    pub verdicts: Vec<(Provider, ProviderVerdict)>,
}

impl FieldAttribution {
    /// The provider that answered, or `None` when nothing did.
    pub fn answered_by(&self) -> Option<&Provider> {
        self.verdicts
            .iter()
            .find(|(_, verdict)| verdict.observed().is_some())
            .map(|(provider, _)| provider)
    }

    /// The source that answered, by name. This is the question the decisive
    /// test asks.
    pub fn source(&self) -> Option<&SourceId> {
        self.answered_by().map(|provider| &provider.source)
    }

    /// The value, in the `Document`'s representation, or `None` when the field
    /// is absent.
    ///
    /// Absent means absent: no source could supply it, and nothing was
    /// substituted. There is no path through this type that produces a value
    /// without a provider that observed it.
    pub fn value(&self) -> Option<&serde_json::Value> {
        self.verdicts
            .iter()
            .find_map(|(_, verdict)| verdict.observed())
    }

    /// The value as text, when it is text.
    pub fn text(&self) -> Option<&str> {
        self.value().and_then(serde_json::Value::as_str)
    }

    /// Whether no declared provider could supply this field.
    pub fn is_absent(&self) -> bool {
        self.value().is_none()
    }

    /// Providers declared **above** the winner that observed nothing — the
    /// reason a lower-precedence source got to answer.
    pub fn silent_above(&self) -> Vec<&Provider> {
        self.verdicts
            .iter()
            .take_while(|(_, verdict)| verdict.observed().is_none())
            .map(|(provider, _)| provider)
            .collect()
    }

    /// Providers declared **below** the winner that also observed a value —
    /// the values the declared order rejected. Kept, because "nothing else had
    /// an answer" and "three sources had answers and this one is declared
    /// first" are different situations with the same winner.
    pub fn outranked(&self) -> Vec<&Provider> {
        self.verdicts
            .iter()
            .skip_while(|(_, verdict)| verdict.observed().is_none())
            .skip(1)
            .filter(|(_, verdict)| verdict.observed().is_some())
            .map(|(provider, _)| provider)
            .collect()
    }

    /// One line saying why this field holds what it holds.
    pub fn explain(&self) -> String {
        let names = |providers: Vec<&Provider>| {
            providers
                .iter()
                .map(|provider| format!("`{provider}`"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        match self.answered_by() {
            None => format!(
                "`{}` is absent: {}",
                self.field,
                self.verdicts
                    .iter()
                    .map(|(provider, verdict)| format!("`{provider}` {}", verdict.as_str()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Some(winner) => {
                let mut line = format!("`{}` answered by `{winner}`", self.field);
                let silent = self.silent_above();
                if !silent.is_empty() {
                    line.push_str(&format!(
                        "; {} declared first but had nothing",
                        names(silent)
                    ));
                }
                let outranked = self.outranked();
                if !outranked.is_empty() {
                    line.push_str(&format!(
                        "; {} also observed a value and is declared lower",
                        names(outranked)
                    ));
                }
                line
            }
        }
    }
}

/// One assembled `Document`: one concept, attributes from several sources,
/// every source row still intact.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Document {
    path: String,
    fields: BTreeMap<AttrName, FieldAttribution>,
    observations: Vec<SourceRow>,
    claims: Vec<IdentityClaim>,
}

impl Document {
    pub(super) fn new(
        path: String,
        fields: BTreeMap<AttrName, FieldAttribution>,
        observations: Vec<SourceRow>,
        claims: Vec<IdentityClaim>,
    ) -> Self {
        Self {
            path,
            fields,
            observations,
            claims,
        }
    }

    /// The natural key: the workspace-relative path every source agreed on.
    ///
    /// A `String`, not an `Option<String>`, because the schema declares `path`
    /// non-nullable and the assembler refuses a row without one.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Every field, in attribute-name order.
    pub fn fields(&self) -> &BTreeMap<AttrName, FieldAttribution> {
        &self.fields
    }

    /// One field's attribution.
    pub fn field(&self, name: &str) -> Option<&FieldAttribution> {
        self.fields.get(&AttrName::from(name))
    }

    /// One field's value, or `None` when no source could supply it.
    pub fn value(&self, name: &str) -> Option<&serde_json::Value> {
        self.field(name).and_then(FieldAttribution::value)
    }

    /// One field's value as text.
    pub fn text(&self, name: &str) -> Option<&str> {
        self.field(name).and_then(FieldAttribution::text)
    }

    /// Which source answered one field, by name.
    pub fn source_of(&self, name: &str) -> Option<&SourceId> {
        self.field(name).and_then(FieldAttribution::source)
    }

    /// The rows the adapters produced, verbatim and unmodified, in the order
    /// the sources were federated. Resolution selected from these; it did not
    /// rewrite them, and none of them was discarded.
    pub fn observations(&self) -> &[SourceRow] {
        &self.observations
    }

    /// The identity claims that say those rows denote this document. One per
    /// observation: each names the source record and the canonical record, and
    /// both keep their ids.
    pub fn claims(&self) -> &[IdentityClaim] {
        &self.claims
    }

    /// Every field's [`FieldAttribution::explain`], in attribute-name order.
    pub fn explain(&self) -> Vec<String> {
        self.fields
            .values()
            .map(FieldAttribution::explain)
            .collect()
    }
}

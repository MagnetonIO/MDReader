use super::*;
use crate::SyncScope;
use serde::Deserialize;
use url::Url;
pub(super) fn validate_sync_request(
    registration: &ReviewedOidcRegistration,
    scope: &SyncScope,
    method: &str,
    exact_url: &str,
    body: &[u8],
) -> AuthorityResult<()> {
    let url = Url::parse(exact_url).map_err(|_| AuthorityError::Scope)?;
    if url.origin() != registration.sync.origin()
        || url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.as_str() != exact_url
        || body.len() > crate::MAX_ENVELOPE_BYTES
    {
        return Err(AuthorityError::Scope);
    }
    if let Some(blob) = url.path().strip_prefix("/v1/blobs/") {
        crate::crypto::validate_digest(blob).map_err(|_| AuthorityError::Scope)?;
        if method == "GET" && url.query().is_none() && body.is_empty() {
            return Ok(());
        }
        if method == "PUT" {
            let pairs: Vec<_> = url.query_pairs().collect();
            if pairs.len() != 2
                || pairs[0].0 != "operationId"
                || pairs[1].0 != "libraryId"
                || pairs[1].1 != scope.library_id()
            {
                return Err(AuthorityError::Scope);
            }
            crate::crypto::validate_uuid(&pairs[0].1).map_err(|_| AuthorityError::Scope)?;
            let expected = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("operationId", &pairs[0].1)
                .append_pair("libraryId", scope.library_id())
                .finish();
            if url.query() != Some(expected.as_str()) {
                return Err(AuthorityError::Scope);
            }
            crate::crypto::validate_envelope(body).map_err(|_| AuthorityError::Scope)?;
            if crate::blob_digest(body) != blob {
                return Err(AuthorityError::Scope);
            }
            return Ok(());
        }
    }
    if method == "GET" && url.path() == "/v1/changes" && body.is_empty() {
        let pairs: Vec<_> = url.query_pairs().collect();
        if pairs.len() == 2 && pairs[0].0 == "after" && pairs[1].0 == "limit" {
            let after = pairs[0]
                .1
                .parse::<u64>()
                .map_err(|_| AuthorityError::Scope)?;
            let limit = pairs[1]
                .1
                .parse::<u64>()
                .map_err(|_| AuthorityError::Scope)?;
            if after <= 9_007_199_254_740_991
                && (1..=100).contains(&limit)
                && pairs[0].1 == after.to_string()
                && pairs[1].1 == limit.to_string()
                && url.query() == Some(format!("after={after}&limit={limit}").as_str())
            {
                return Ok(());
            }
        }
    }
    if method == "POST" && url.query().is_none() {
        match url.path() {
            "/v1/commit" => {
                let operation: crate::CommitOperation =
                    serde_json::from_slice(body).map_err(|_| AuthorityError::Scope)?;
                operation.wire_bytes().map_err(|_| AuthorityError::Scope)?;
                return Ok(());
            }
            "/v1/ack" => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Ack {
                    cursor: u64,
                }
                let ack: Ack = serde_json::from_slice(body).map_err(|_| AuthorityError::Scope)?;
                if ack.cursor <= 9_007_199_254_740_991 {
                    return Ok(());
                }
            }
            _ => {}
        }
    }
    Err(AuthorityError::LifecycleGated)
}

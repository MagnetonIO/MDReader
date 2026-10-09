use super::*;
use std::time::{Duration, Instant};

#[test]
fn adaptive_pages_reserve_escaped_tokens_and_actual_signed_envelope() {
    let root = RecordKey::new("r".repeat(256)).unwrap();
    let binding = SourceBinding::new(
        LibraryId::new("\\".repeat(256)).unwrap(),
        SourceId::new("\"".repeat(256)).unwrap(),
        AccountBinding::new("\\".repeat(256)).unwrap(),
        uuid::Uuid::new_v4(),
        "a".repeat(64),
    )
    .unwrap();
    let codec = CursorCodec::new([7; 32]);
    let context = IoContext::new(
        uuid::Uuid::new_v4(),
        Instant::now() + Duration::from_secs(10),
        MAX_PAGE_ROWS,
        1024 * 1024,
        Cancellation::default(),
    )
    .unwrap();
    let mut walk = Walk {
        v: 3,
        root: root.as_str().into(),
        stack: vec![Frame::new(root.as_str().into())],
    };
    for i in 0..20 {
        walk.stack[0]
            .pending
            .push_back(format!("{i:03}{}", "a".repeat(253)));
    }
    walk.validate(&root).unwrap();
    let rows = walk.page_rows(&context, &codec, &binding).unwrap();
    assert!(rows > 0 && rows < MAX_PAGE_ROWS);
    let frame = walk.stack.last_mut().unwrap();
    for i in 0..rows {
        frame
            .pending
            .push_back(format!("{i:03}{}", "b".repeat(253)));
    }
    // Two layers of JSON escaping, followed by base64 and the source binding.
    frame.token = "\\".repeat(2048);
    walk.advance().unwrap();
    walk.validate(&root).unwrap();
    let cursor = walk.cursor(&codec, &binding).unwrap();
    assert!(cursor.as_bytes().len() <= MAX_SIGNED_CURSOR_BYTES);
    let decoded = codec
        .decode_traversal(
            ProviderKind::GoogleDrive,
            &binding,
            CursorKind::List,
            &cursor,
        )
        .unwrap();
    let reopened: Walk = serde_json::from_str(&decoded).unwrap();
    reopened.validate(&root).unwrap();
    assert_eq!(reopened.stack[0].token.len(), 2048);
    assert_eq!(reopened.pending(), rows + 19);
}

//! Authenticated one-shot local transport; no owner capability leaves the server.
use super::*;
mod discovery;
pub(super) use discovery::{
    prepare as prepare_directory, publish as publish_endpoint, read as read_endpoint,
};

pub(super) fn connection(
    mut stream: TcpStream,
    token: &str,
    runtime: &runtime::Runtime,
) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    let input = read_frame(&mut BufReader::new(stream.try_clone()?), MAX_REQUEST)?;
    let envelope: Envelope =
        serde_json::from_slice(&input).map_err(|_| protocol_error("invalid request envelope"))?;
    let control = matches!(envelope.request, Request::Preservation { .. });
    let result = if envelope.protocol != PROTOCOL || !token_matches(&envelope.token, token) {
        Err(ReaderError::Unavailable)
    } else if control && input.len() as u64 > preservation::MAX_CONTROL_BYTES {
        serde_json::to_value(preservation::ControlReply::Failure {
            error: preservation::Error::Bounds,
        })
        .map_err(|_| protocol_error("response serialization failed"))
    } else {
        runtime.execute(envelope.request)
    };
    let reply = match result {
        Ok(result) => Reply {
            result,
            error: None,
        },
        Err(error) => Reply {
            result: Value::Null,
            error: Some(wire_error(error)),
        },
    };
    let mut output = encode(&reply)?;
    let maximum = if control {
        preservation::MAX_CONTROL_BYTES
    } else {
        MAX_RESPONSE
    };
    if output.len() as u64 > maximum {
        return Err(protocol_error("response exceeds budget"));
    }
    output.push(b'\n');
    let result=stream.write_all(&output).map_err(ReaderError::Io);
    // Wake even after a lost shutdown reply. Never leave a held shutdown intent
    // dependent on the caller issuing another request or retrying publication.
    runtime.wake_shutdown_if_requested();
    result
}

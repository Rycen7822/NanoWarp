use super::{Error, decode_response_event, endpoint_url};
use base64::Engine as _;
use base64::prelude::BASE64_URL_SAFE;
use prost::Message as _;

#[test]
fn routes_regular_requests_to_multi_agent_endpoint() {
    let prefix = if cfg!(feature = "agent_mode_evals") {
        "agent-mode-evals"
    } else {
        "ai"
    };

    assert!(endpoint_url().ends_with(&format!("/{prefix}/multi-agent")));
}

#[test]
fn decodes_quoted_base64_protobuf_response_event() {
    let expected = warp_multi_agent_api::ResponseEvent::default();
    let encoded = BASE64_URL_SAFE.encode(expected.encode_to_vec());

    let decoded = decode_response_event(&format!("\"{encoded}\"")).unwrap();

    assert_eq!(decoded, expected);
}

#[test]
fn distinguishes_base64_and_protobuf_decode_errors() {
    assert!(matches!(
        decode_response_event("%"),
        Err(Error::Base64Decode(_))
    ));

    let invalid_protobuf = BASE64_URL_SAFE.encode([0xff]);
    assert!(matches!(
        decode_response_event(&invalid_protobuf),
        Err(Error::ProtobufDecode(_))
    ));
}

#[cfg(not(target_family = "wasm"))]
#[test]
fn native_output_stream_is_send() {
    fn assert_send<T: Send>() {}

    assert_send::<super::OutputStream>();
}

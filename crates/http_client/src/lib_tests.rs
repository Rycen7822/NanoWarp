use super::*;

#[test]
fn external_client_omits_automatic_headers_and_preserves_signing_headers() {
    let client = Client::new();
    let url = format!(
        "{}/object?X-Amz-Signature=test%2Bsignature&x-id=PutObject",
        ChannelState::server_root_url()
    );
    let api_request = client.get(&url).build().unwrap().wrapped;
    assert!(
        api_request
            .headers()
            .contains_key(headers::WARP_OS_CATEGORY)
    );

    let external_client = Client::new_external();
    let request = external_client
        .put(&url)
        .header("x-amz-checksum-crc32c", "EjRWeA==")
        .header(AUTHORIZATION, "AWS4-HMAC-SHA256 test-signature")
        .build()
        .unwrap()
        .wrapped;
    assert_eq!(request.url().as_str(), url);
    assert_eq!(request.headers().len(), 2);
    assert_eq!(request.headers()["x-amz-checksum-crc32c"], "EjRWeA==");
    assert_eq!(
        request.headers()[AUTHORIZATION],
        "AWS4-HMAC-SHA256 test-signature"
    );
}

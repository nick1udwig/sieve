use crate::{canonicalize_net_origin_scope, canonicalize_sink_key};

#[test]
fn canonicalizes_url_sink_keys() {
    let sink = canonicalize_sink_key("HTTPS://Api.Example.Com:443/v1/../v1/%7euser?q=1#frag")
        .expect("canonicalization");
    assert_eq!(sink, "https://api.example.com/v1/~user");

    let sink2 = canonicalize_sink_key("http://EXAMPLE.com:80").expect("canonicalization");
    assert_eq!(sink2, "http://example.com/");
}

#[test]
fn canonicalizes_net_origin_scopes() {
    assert_eq!(
        canonicalize_net_origin_scope("HTTPS://Api.Example.Com:443/v1/path?q=1#frag"),
        Some("https://api.example.com".to_string())
    );
    assert_eq!(
        canonicalize_net_origin_scope("http://EXAMPLE.com:8080/path"),
        Some("http://example.com:8080".to_string())
    );
    assert_eq!(canonicalize_net_origin_scope("not-a-url"), None);
}

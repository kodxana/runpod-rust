//! Ports of tests/test_api/test_rest.py and endpoint authentication tests.
use runpod::{Client, ClientConfig, Error, json};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_json, header, method, path, query_param},
};

async fn setup() -> (MockServer, Client) {
    let server = MockServer::start().await;
    let client = Client::with_config(ClientConfig {
        api_base_url: format!("{}/prefix/", server.uri()),
        endpoint_base_url: format!("{}/v2", server.uri()),
        ..ClientConfig::new("key")
    })
    .unwrap();
    (server, client)
}

#[tokio::test]
async fn test_authenticated_mutation_at_custom_base_url() {
    let (server, client) = setup().await;
    Mock::given(method("POST"))
        .and(path("/prefix/v2/pods"))
        .and(header("Authorization", "Bearer key"))
        .and(query_param("include", "all"))
        .and(body_json(json!({"name":"pod"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id":"pod-1"})))
        .expect(1)
        .mount(&server)
        .await;
    let result = client
        .run_rest_request(
            "POST",
            "/v2/pods",
            Some(&json!({"name":"pod"})),
            &[("include", "all")],
        )
        .await
        .unwrap();
    assert_eq!(result, json!({"id":"pod-1"}));
}

#[test]
fn test_missing_key_fails_before_network_request() {
    assert!(matches!(Client::new(""), Err(Error::Authentication(_))));
}

#[tokio::test]
async fn test_invalid_key_raises_authentication_error() {
    let (server, client) = setup().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401))
        .expect(1)
        .mount(&server)
        .await;
    assert!(matches!(
        client.run_rest_request("GET", "v2/pods", None, &[]).await,
        Err(Error::Authentication(_))
    ));
}

#[tokio::test]
async fn test_empty_response_returns_none() {
    for status in [200, 204] {
        let (server, client) = setup().await;
        Mock::given(method("DELETE"))
            .respond_with(ResponseTemplate::new(status))
            .expect(1)
            .mount(&server)
            .await;
        assert!(
            client
                .run_rest_request("DELETE", "v2/pods", None, &[])
                .await
                .unwrap()
                .is_null()
        );
    }
}

#[tokio::test]
async fn test_problem_json_preserves_validation_error() {
    let (server, client) = setup().await;
    Mock::given(method("POST")).respond_with(ResponseTemplate::new(422).set_body_json(json!({
        "title":"Unprocessable Entity", "detail":"request validation failed", "errors":["$.name is required"]
    }))).expect(1).mount(&server).await;
    let error = client
        .run_rest_request("POST", "/v2/pods", Some(&json!({})), &[])
        .await
        .unwrap_err();
    match error {
        Error::Query {
            message,
            query,
            status_code,
            errors,
        } => {
            assert_eq!(message, "request validation failed");
            assert_eq!(query, "POST /v2/pods");
            assert_eq!(status_code, Some(422));
            assert_eq!(*errors, json!(["$.name is required"]));
        }
        other => panic!("expected query error, got {other:?}"),
    }
}

#[tokio::test]
async fn test_error_body_fallbacks_do_not_mask_http_failure() {
    for (body, expected) in [
        ("{\"title\":\"service unavailable\"}", "service unavailable"),
        ("upstream failure", "upstream failure"),
        ("[\"unexpected array\"]", "[\"unexpected array\"]"),
        ("", "Request failed with status 503"),
    ] {
        let (server, client) = setup().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503).set_body_string(body))
            .expect(1)
            .mount(&server)
            .await;
        let error = client
            .run_rest_request("POST", "/v2/pods", Some(&json!({})), &[])
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), expected);
    }
}

#[tokio::test]
async fn test_pagination_collects_pages_and_rejects_repeated_cursor() {
    let (server, client) = setup().await;
    Mock::given(path("/prefix/v2/pods"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"pods":[{"id":"one"}],"pagination":{"hasNextPage":true,"nextCursor":"next"}}),
        ))
        .expect(1)
        .with_priority(10)
        .mount(&server)
        .await;
    Mock::given(query_param("cursor", "next"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"pods":[{"id":"two"}],"pagination":{"hasNextPage":false}})),
        )
        .expect(1)
        .with_priority(1)
        .mount(&server)
        .await;
    assert_eq!(
        client
            .run_rest_paginated_request("/v2/pods", "pods")
            .await
            .unwrap(),
        vec![json!({"id":"one"}), json!({"id":"two"})]
    );
    server.reset().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"pods":[],"pagination":{"hasNextPage":true,"nextCursor":"same"}}),
        ))
        .expect(2)
        .mount(&server)
        .await;
    assert!(
        client
            .run_rest_paginated_request("/v2/pods", "pods")
            .await
            .unwrap_err()
            .to_string()
            .contains("repeated a cursor")
    );
}

#[tokio::test]
async fn test_graphql_error_and_variables() {
    let (server, client) = setup().await;
    Mock::given(path("/prefix/graphql"))
        .and(body_json(
            json!({"query":"query Test", "variables":{"x":1}}),
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"errors":[{"message":"invalid query"}]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(
        client
            .run_graphql_query("query Test", Some(&json!({"x":1})))
            .await
            .unwrap_err()
            .to_string(),
        "invalid query"
    );
}

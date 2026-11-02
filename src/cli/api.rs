use crate::{Client, Error, Result, Value};

pub(super) async fn list_all(
    client: &Client,
    path: &str,
    collection: &str,
    params: &[(&str, &str)],
) -> Result<Vec<Value>> {
    let mut query: Vec<(String, String)> = params
        .iter()
        .map(|(k, v)| ((*k).into(), (*v).into()))
        .collect();
    let mut rows = Vec::new();
    loop {
        let borrowed: Vec<_> = query
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let mut page = client
            .run_rest_request("GET", path, None, &borrowed)
            .await?;
        let records = page
            .get_mut(collection)
            .and_then(Value::as_array_mut)
            .ok_or_else(|| Error::InvalidResponse(format!("missing array {collection}")))?;
        rows.append(records);
        let pagination = &page["pagination"];
        let has_next = pagination["hasNextPage"]
            .as_bool()
            .ok_or_else(|| Error::InvalidResponse("missing pagination.hasNextPage".into()))?;
        if !has_next {
            return Ok(rows);
        }
        let cursor = pagination["nextCursor"]
            .as_str()
            .ok_or_else(|| Error::InvalidResponse("missing pagination.nextCursor".into()))?;
        query.retain(|(key, _)| key != "cursor");
        query.push(("cursor".into(), cursor.into()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ClientConfig, cli::CliError, json};
    use std::time::Duration;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    #[tokio::test]
    async fn test_list_all_follows_cursors() {
        let server = MockServer::start().await;
        for response in [
            json!({"pods": [{"id":"a"}], "pagination":{"hasNextPage":true, "nextCursor":"c1"}}),
            json!({"pods": [{"id":"b"}], "pagination":{"hasNextPage":false, "nextCursor":null}}),
        ] {
            Mock::given(method("GET"))
                .and(path("/v2/pods"))
                .respond_with(ResponseTemplate::new(200).set_body_json(response))
                .up_to_n_times(1)
                .expect(1)
                .mount(&server)
                .await;
        }
        let client = Client::with_config(ClientConfig {
            api_base_url: server.uri(),
            ..ClientConfig::new("key")
        })
        .unwrap();
        let params = [("limit", "1")];
        assert_eq!(
            list_all(&client, "/v2/pods", "pods", &params)
                .await
                .unwrap(),
            vec![json!({"id":"a"}), json!({"id":"b"})]
        );
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests[0].url.query(), Some("limit=1"));
        assert_eq!(requests[1].url.query(), Some("limit=1&cursor=c1"));
        assert_eq!(params, [("limit", "1")]);
    }

    #[tokio::test]
    async fn test_api_errors_are_click_errors() {
        let error = CliError::from(Error::Authentication("unauthorized".into()));
        assert_eq!(error.to_string(), "unauthorized");
        assert_eq!(error.code(), 1);
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/slow"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
            .mount(&server)
            .await;
        let client = Client::with_config(ClientConfig {
            api_base_url: server.uri(),
            timeout: Duration::from_millis(30),
            ..ClientConfig::new("key")
        })
        .unwrap();
        let error = CliError::from(
            client
                .run_rest_request("GET", "/slow", None, &[])
                .await
                .unwrap_err(),
        );
        assert_eq!(error.code(), 1);
        assert!(matches!(error, CliError::Api(Error::Http(ref source)) if source.is_timeout()));
    }
}

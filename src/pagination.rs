use crate::{Client, Error, Result, Value};

pub(crate) async fn list_all(
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

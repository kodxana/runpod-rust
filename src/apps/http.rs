//! Native counterpart of sdk-runtime's deployed class-API adapter.
use super::{FunctionArgs, NativeApi, RouteSpec};
use crate::{Error, Result, Value, http_docs, json};
use axum::{
    Json, Router,
    body::to_bytes,
    extract::{Request, State},
    http::{Method, StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
};
use regex::Regex;
use std::{
    collections::BTreeSet,
    future::Future,
    sync::{Arc, LazyLock},
};
use tokio::net::TcpListener;

struct CompiledRoute {
    spec: RouteSpec,
    pattern: Regex,
    openapi_path: String,
}

impl CompiledRoute {
    fn new(spec: RouteSpec) -> Result<Self> {
        static PARAMETER: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"\{([a-zA-Z_][a-zA-Z0-9_]*)(?::([a-zA-Z_][a-zA-Z0-9_]*))?\}").unwrap()
        });
        let mut pattern = String::from("^");
        let mut openapi_path = String::new();
        let mut names = BTreeSet::new();
        let mut duplicates = BTreeSet::new();
        let mut offset = 0;
        for parameter in PARAMETER.captures_iter(&spec.path) {
            let token = parameter.get(0).unwrap();
            let name = parameter.get(1).unwrap().as_str();
            if !names.insert(name) {
                duplicates.insert(name);
            }
            let kind = parameter.get(2).map_or("str", |m| m.as_str());
            let fragment = match kind {
                "str" => "[^/]+",
                "int" => "[0-9]+",
                "float" => "[0-9]+(?:\\.[0-9]+)?",
                "uuid" => {
                    "[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}"
                }
                "path" => ".*",
                _ => {
                    return Err(Error::InvalidInput(format!(
                        "Unknown path convertor '{kind}'"
                    )));
                }
            };
            pattern.push_str(&regex::escape(&spec.path[offset..token.start()]));
            pattern.push_str(fragment);
            openapi_path.push_str(&spec.path[offset..token.start()]);
            openapi_path.push('{');
            openapi_path.push_str(name);
            openapi_path.push('}');
            offset = token.end();
        }
        if !duplicates.is_empty() {
            return Err(Error::InvalidInput(format!(
                "Duplicated param name{} {} at path {}",
                if duplicates.len() > 1 { "s" } else { "" },
                duplicates.into_iter().collect::<Vec<_>>().join(", "),
                spec.path
            )));
        }
        pattern.push_str(&regex::escape(&spec.path[offset..]));
        pattern.push('$');
        openapi_path.push_str(&spec.path[offset..]);
        let pattern =
            Regex::new(&pattern).map_err(|error| Error::InvalidInput(error.to_string()))?;
        Ok(Self {
            spec,
            pattern,
            openapi_path,
        })
    }
}

struct ApiState {
    title: String,
    api: NativeApi,
    routes: Vec<CompiledRoute>,
    schema: Value,
}

pub(super) async fn serve(
    title: String,
    api: NativeApi,
    listener: TcpListener,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let mut shutdown = Box::pin(shutdown);
    if let Some(factory) = api.build_factory() {
        let app = tokio::select! {
            result = factory => result?,
            () = &mut shutdown => return Ok(()),
        };
        return app.serve(listener, shutdown).await;
    }
    let routes = api
        .http_routes()?
        .into_iter()
        .map(CompiledRoute::new)
        .collect::<Result<Vec<_>>>()?;
    let schema = openapi(&title, &routes);
    tokio::select! {
        result = api.initialize() => result?,
        () = &mut shutdown => return Ok(()),
    }
    let state = Arc::new(ApiState {
        title,
        api,
        routes,
        schema,
    });
    axum::serve(listener, Router::new().fallback(dispatch).with_state(state))
        .with_graceful_shutdown(shutdown)
        .await?;
    Ok(())
}

#[derive(Clone, Copy)]
enum Core {
    Schema,
    Docs,
    OAuth,
    Redoc,
    Ping,
}

fn core(path: &str) -> Option<Core> {
    match path {
        "/openapi.json" => Some(Core::Schema),
        "/docs" => Some(Core::Docs),
        "/docs/oauth2-redirect" => Some(Core::OAuth),
        "/redoc" => Some(Core::Redoc),
        "/ping" => Some(Core::Ping),
        _ => None,
    }
}

async fn dispatch(State(state): State<Arc<ApiState>>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let path = percent_encoding::percent_decode_str(parts.uri.path()).decode_utf8_lossy();
    let mut partial = None;
    if let Some(route) = core(&path) {
        let head = !matches!(route, Core::Ping);
        if parts.method == Method::GET || (head && parts.method == Method::HEAD) {
            return match route {
                Core::Schema => Json(state.schema.clone()).into_response(),
                Core::Docs => Html(http_docs::swagger(&state.title)).into_response(),
                Core::Redoc => Html(http_docs::redoc(&state.title)).into_response(),
                Core::OAuth => Html(http_docs::OAUTH).into_response(),
                Core::Ping => Json(json!({"status":"healthy"})).into_response(),
            };
        }
        partial = Some(if head { "GET, HEAD" } else { "GET" });
    }
    for route in &state.routes {
        if !route.pattern.is_match(&path) {
            continue;
        }
        if parts.method.as_str() != route.spec.method {
            partial.get_or_insert(&route.spec.method);
            continue;
        }
        let mut args = FunctionArgs::default();
        if parts.method != Method::GET
            && let Ok(bytes) = to_bytes(body, usize::MAX).await
            && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
            && !value.is_null()
        {
            args.args.push(value);
        }
        return match state
            .api
            .call_http_handler(&route.spec.handler_name, args)
            .await
        {
            Ok(response) => response,
            Err(error) => {
                tracing::error!(%error, handler=%route.spec.handler_name, "API handler failed");
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response()
            }
        };
    }
    if let Some(allow) = partial {
        return (
            StatusCode::METHOD_NOT_ALLOWED,
            [(header::ALLOW, allow)],
            Json(json!({"detail":"Method Not Allowed"})),
        )
            .into_response();
    }
    if path != "/" {
        let alternate = if path.ends_with('/') {
            path.trim_end_matches('/').to_string()
        } else {
            format!("{path}/")
        };
        if core(&alternate).is_some() || state.routes.iter().any(|r| r.pattern.is_match(&alternate))
        {
            let raw = parts.uri.path();
            let raw = if path.ends_with('/') {
                raw.trim_end_matches('/').to_owned()
            } else {
                format!("{raw}/")
            };
            let query = parts
                .uri
                .query()
                .map(|q| format!("?{q}"))
                .unwrap_or_default();
            let location = match parts
                .headers
                .get(header::HOST)
                .and_then(|v| v.to_str().ok())
            {
                Some(host) => format!(
                    "{}://{host}{raw}{query}",
                    parts.uri.scheme_str().unwrap_or("http")
                ),
                None => format!("{raw}{query}"),
            };
            return Redirect::temporary(&location).into_response();
        }
    }
    (StatusCode::NOT_FOUND, Json(json!({"detail":"Not Found"}))).into_response()
}

fn openapi(title: &str, routes: &[CompiledRoute]) -> Value {
    let mut schema = json!({"openapi":"3.1.0","info":{"title":title,"version":"0.1.0"},"paths":{}});
    schema["paths"]["/ping"] = json!({"get":operation("ping", "/ping", "GET")});
    for route in routes {
        let path = &route.openapi_path;
        if schema["paths"].get(path).is_none() {
            schema["paths"][path] = json!({});
        }
        schema["paths"][path][route.spec.method.to_lowercase()] =
            operation(&route.spec.handler_name, path, &route.spec.method);
    }
    schema
}

fn operation(name: &str, path: &str, method: &str) -> Value {
    let mut summary = String::new();
    let mut initial = true;
    for ch in name.replace('_', " ").chars() {
        if ch.is_alphabetic() {
            if initial {
                summary.extend(ch.to_uppercase());
            } else {
                summary.extend(ch.to_lowercase());
            }
            initial = false;
        } else {
            summary.push(ch);
            initial = true;
        }
    }
    let id: String = format!("{name}{path}")
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    json!({"summary":summary,"operationId":format!("{id}_{}", method.to_lowercase()),"responses":{"200":{"description":"Successful Response","content":{"application/json":{"schema":{}}}}}})
}

use crate::{Result, json};
use axum::{
    Json, Router,
    extract::Request,
    http::{Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use futures_util::future::BoxFuture;
use std::{fmt, future::Future};
use tokio::net::TcpListener;
use tower::ServiceExt;

/// Health-route behavior for a native application factory.
///
/// Axum does not expose its route inventory. A factory therefore explicitly declares
/// whether it already owns `/ping`, replacing the Python runtime's route inspection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactoryHealth {
    /// The app has no `/ping` route: supply GET health and return 405 for other methods.
    Add,
    /// Preserve the app's existing `/ping` route, including its accepted methods and status.
    Preserve,
    /// Adapt an opaque service: answer every HTTP method at `/ping` before delegating.
    Wrap,
}

/// An owned native application produced by [`super::NativeApi::http_factory`].
///
/// The router owns request parsing, middleware, and response types. Arbitrary Tower
/// services can be attached with `Router::fallback_service`. The asynchronous factory
/// performs startup before returning this value; a shutdown hook runs after requests
/// drain. Neither callback is invoked during resource registration.
pub struct NativeHttpApp {
    router: Router,
    health: FactoryHealth,
    shutdown: Option<BoxFuture<'static, Result<()>>>,
}

impl fmt::Debug for NativeHttpApp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NativeHttpApp")
            .field("health", &self.health)
            .field("has_shutdown", &self.shutdown.is_some())
            .finish_non_exhaustive()
    }
}

impl NativeHttpApp {
    /// Accepts a ready router and its explicit health-route policy.
    #[must_use]
    pub fn new(router: Router, health: FactoryHealth) -> Self {
        Self {
            router,
            health,
            shutdown: None,
        }
    }

    /// Registers asynchronous cleanup after graceful shutdown or a serving error.
    ///
    /// Replaces a previous hook. Cancellation by dropping the server future does not
    /// run asynchronous cleanup; use owned RAII guards for cancellation-safe resources.
    #[must_use]
    pub fn on_shutdown(
        mut self,
        shutdown: impl Future<Output = Result<()>> + Send + 'static,
    ) -> Self {
        self.shutdown = Some(Box::pin(shutdown));
        self
    }

    pub(super) async fn serve(
        self,
        listener: TcpListener,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> Result<()> {
        let router = self.router;
        let health = self.health;
        let router = if health == FactoryHealth::Preserve {
            router
        } else {
            Router::new().fallback(move |request: Request| {
                let router = router.clone();
                async move {
                    if percent_encoding::percent_decode_str(request.uri().path())
                        .decode_utf8_lossy()
                        == "/ping"
                    {
                        return ping(health, request.method());
                    }
                    match router.oneshot(request).await {
                        Ok(response) => response,
                        Err(error) => match error {},
                    }
                }
            })
        };
        let served = axum::serve(listener, router)
            .with_graceful_shutdown(shutdown)
            .await;
        let cleaned = match self.shutdown {
            Some(shutdown) => shutdown.await,
            None => Ok(()),
        };
        if let Err(error) = served {
            if let Err(cleanup) = cleaned {
                tracing::error!(%cleanup, "API factory cleanup also failed");
            }
            return Err(error.into());
        }
        cleaned
    }
}

fn ping(health: FactoryHealth, method: &Method) -> Response {
    if health == FactoryHealth::Wrap {
        // The opaque ASGI wrapper uses json.dumps spacing rather than JSONResponse.
        return (
            [(header::CONTENT_TYPE, "application/json")],
            r#"{"status": "healthy"}"#,
        )
            .into_response();
    }
    if method == Method::GET {
        Json(json!({"status":"healthy"})).into_response()
    } else {
        (
            StatusCode::METHOD_NOT_ALLOWED,
            [(header::ALLOW, "GET")],
            Json(json!({"detail":"Method Not Allowed"})),
        )
            .into_response()
    }
}

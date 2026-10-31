//! Shared HTML for the native development and application API servers.
fn escape(title: &str) -> String {
    title
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub(crate) fn swagger(title: &str) -> String {
    format!(
        r#"<!doctype html><html><head><title>{} - Swagger UI</title><link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5/swagger-ui.css"></head><body><div id="swagger-ui"></div><script src="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5/swagger-ui-bundle.js"></script><script>SwaggerUIBundle({{url:'/openapi.json',dom_id:'#swagger-ui',deepLinking:true,oauth2RedirectUrl:window.location.origin+'/docs/oauth2-redirect',presets:[SwaggerUIBundle.presets.apis,SwaggerUIBundle.SwaggerUIStandalonePreset]}})</script></body></html>"#,
        escape(title)
    )
}

pub(crate) fn redoc(title: &str) -> String {
    format!(
        r#"<!doctype html><html><head><title>{} - ReDoc</title></head><body><redoc spec-url="/openapi.json"></redoc><script src="https://cdn.jsdelivr.net/npm/redoc@2/bundles/redoc.standalone.js"></script></body></html>"#,
        escape(title)
    )
}

pub(crate) const OAUTH: &str = include_str!("serverless/http/oauth.html");

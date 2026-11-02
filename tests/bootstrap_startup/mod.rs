use runpod::{
    Result, Value,
    apps::{
        App, AppRegistry, NativeApi, NativeFunction, ResourceSpec, RouteSpec,
        deploy::SymbolReference, runtime::DeployedContext, targets::FunctionDefinition,
    },
    json,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub fn register(module: &str, context: &DeployedContext) -> Result<AppRegistry> {
    assert_eq!(module, "user");
    let asset = std::fs::read_to_string(context.app_dir().join("payload.txt"))?;
    std::fs::write(
        std::path::Path::new(&std::env::var_os("RUNPOD_RUST_BOOTSTRAP_ROOT").unwrap())
            .join("loaded"),
        "yes",
    )?;
    let mounted = context.mounts().clone();
    let initialized = Arc::new(AtomicBool::new(false));
    let ready = initialized.clone();
    let init = NativeFunction::sync(move |_| {
        ready.store(true, Ordering::SeqCst);
        Ok(Value::Null)
    });
    let body = NativeFunction::sync(move |call| {
        assert!(initialized.load(Ordering::SeqCst));
        let value = call
            .args
            .first()
            .cloned()
            .or_else(|| call.kwargs.get("value").cloned())
            .unwrap_or(json!(42));
        Ok(json!({"value":value,"init":true,"mount":mounted.path("network","data")?,"asset":asset}))
    });
    let app = App::new("example")?;
    let symbol = SymbolReference {
        module: "user".into(),
        qualname: "work".into(),
    };
    match std::env::var("RUNPOD_RUNTIME_KIND").unwrap().as_str() {
        "api" => {
            let mut api = NativeApi::new("work");
            api.route(
                RouteSpec {
                    method: "GET".into(),
                    path: "/value".into(),
                    handler_name: "value".into(),
                },
                body,
            )?;
            api.init("init", init)?;
            api.set_symbol(symbol);
            app.api(ResourceSpec::default(), api)?;
        }
        kind => {
            let definition = FunctionDefinition {
                name: "work".into(),
                parameters: vec![],
                source: None,
            };
            let handle = if kind == "task" {
                app.task(ResourceSpec::default(), definition, body)?
            } else {
                app.queue(ResourceSpec::default(), definition, body)?
            };
            handle.set_symbol(symbol)?;
            handle.init(init)?;
        }
    }
    let mut registry = AppRegistry::new();
    registry.register(app);
    Ok(registry)
}

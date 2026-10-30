// A separate integration binary keeps log-subscriber setup independent of concurrent
// archive tests that initialize the same tracing callsite without a subscriber.
use runpod::{
    Value,
    apps::deploy::{PackageOptions, package_project},
    json,
};
use std::{
    fs,
    path::Path,
    sync::{Arc, Mutex},
};

fn write_files(root: &Path, files: &Value) {
    for (name, content) in files.as_object().unwrap() {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content.as_str().unwrap()).unwrap();
    }
}

fn archive_names(path: &Path) -> Vec<String> {
    let mut archive =
        tar::Archive::new(flate2::read::GzDecoder::new(fs::File::open(path).unwrap()));
    archive
        .entries()
        .unwrap()
        .map(|entry| {
            entry
                .unwrap()
                .path()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

#[derive(Clone)]
struct Logs(Arc<Mutex<Vec<String>>>);
impl tracing::Subscriber for Logs {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        struct Fields(String);
        impl tracing::field::Visit for Fields {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                use std::fmt::Write as _;
                write!(self.0, "{field}={value:?} ").unwrap();
            }
        }
        let mut fields = Fields(String::new());
        event.record(&mut fields);
        self.0.lock().unwrap().push(fields.0);
    }
}

#[test]
fn test_excluded_paths_are_logged_without_contents() {
    let root = tempfile::tempdir().unwrap();
    let sensitive = "sensitive-file-contents";
    write_files(
        root.path(),
        &json!({".env": sensitive, "private.key": sensitive,
        "private.pem": sensitive, "local.txt": sensitive, ".aws/config": sensitive,
        ".runpodignore": "!*.key\n!.aws/\nlocal.txt\n"}),
    );
    let logs = Logs(Arc::default());
    tracing::subscriber::with_default(logs.clone(), || {
        let artifact =
            package_project(root.path(), &json!({}), &PackageOptions::default()).unwrap();
        assert_eq!(
            archive_names(artifact.path()),
            [".runpodignore", "runpod_manifest.json"]
        );
    });
    let logs = logs.0.lock().unwrap().join("\n");
    for path in [".env", "private.key", "private.pem", "local.txt", ".aws/"] {
        assert!(
            logs.contains(&format!("path={path} ")),
            "missing exclusion for {path}: {logs}"
        );
    }
    assert!(!logs.contains(sensitive));
}

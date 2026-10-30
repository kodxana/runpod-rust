use base64::{Engine as _, engine::general_purpose::STANDARD};
use runpod::{
    Value,
    apps::deploy::{PackageOptions, package_project},
    json,
};
use std::{collections::BTreeMap, fs, io::Read, path::Path};

fn write_files(root: &Path, files: &Value) {
    for (name, content) in files.as_object().unwrap() {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content.as_str().unwrap()).unwrap();
    }
}

fn read_archive(path: &Path) -> (Vec<String>, BTreeMap<String, Vec<u8>>) {
    let input = flate2::read::GzDecoder::new(fs::File::open(path).unwrap());
    let mut archive = tar::Archive::new(input);
    let mut names = Vec::new();
    let mut contents = BTreeMap::new();
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        assert!(entry.header().entry_type().is_file());
        let name = entry.path().unwrap().to_string_lossy().replace('\\', "/");
        let mut data = Vec::new();
        entry.read_to_end(&mut data).unwrap();
        names.push(name.clone());
        assert!(contents.insert(name, data).is_none());
    }
    (names, contents)
}

fn python_case(name: &str) {
    let fixtures: Vec<Value> = serde_json::from_str(include_str!("fixtures/package.json")).unwrap();
    let fixture = fixtures.iter().find(|case| case["name"] == name).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    fs::create_dir(&project).unwrap();
    write_files(&project, &fixture["files"]);
    let env_dir = if fixture["env_files"].is_object() {
        let path = project.join(fixture["env_path"].as_str().unwrap());
        fs::create_dir_all(&path).unwrap();
        write_files(&path, &fixture["env_files"]);
        Some(path)
    } else {
        None
    };
    let artifact = package_project(
        &project,
        &fixture["manifest"],
        &PackageOptions {
            output: Some(project.join(fixture["output"].as_str().unwrap())),
            env_dir,
        },
    )
    .unwrap();
    let (names, mut contents) = read_archive(artifact.path());
    assert_eq!(json!(names), fixture["names"], "{name}");
    let manifest: Value =
        serde_json::from_slice(&contents.remove("runpod_manifest.json").unwrap()).unwrap();
    assert_eq!(manifest, fixture["archived_manifest"], "{name}");
    let encoded: BTreeMap<_, _> = contents
        .into_iter()
        .map(|(name, data)| (name, STANDARD.encode(data)))
        .collect();
    assert_eq!(json!(encoded), fixture["contents"], "{name}");
    let output = artifact.path().to_owned();
    drop(artifact);
    assert!(
        output.exists(),
        "explicit destination remains owned by the caller"
    );
}

macro_rules! python_cases {
    ($($test:ident => $case:literal),+ $(,)?) => { $(
        #[test]
        fn $test() { python_case($case); }
    )+ };
}
python_cases! {
    test_tarball_contains_source_and_manifest => "source_and_manifest",
    test_ignores_and_non_overridable_credentials => "protected_cannot_be_unignored",
    test_credential_like_source_paths_are_included => "credential_like_source_names",
    test_project_ignore_precedence => "nested_precedence",
    defaults_can_be_overridden_except_protected_root_env => "defaults_and_unignore",
    ignored_directories_are_pruned_before_loading_child_rules => "directory_pruning",
    root_anchored_patterns_do_not_match_nested_names => "root_anchoring",
    nested_gitignore_patterns_are_anchored_to_their_own_directory => "nested_root_anchoring",
    character_classes_double_stars_and_negation_match_python => "glob_patterns",
    escaped_comments_negations_and_spaces_match_python => "escaped_literals",
    nested_runpodignore_is_source_without_rule_precedence => "runpodignore_root_scope",
    test_vendored_env_included_under_env => "env_is_separate",
    external_dependency_tree_uses_the_same_env_namespace => "env_outside_project",
    empty_project_still_has_a_manifest => "empty_project",
    unicode_contents_and_depth_first_member_order_match_python => "unicode_and_order",
    output_archive_excludes_itself_even_when_unignored => "output_excluded_by_identity",
}

#[test]
fn automatic_artifact_lifetime_and_persist_are_explicit() {
    let project = tempfile::tempdir().unwrap();
    let artifact = package_project(project.path(), &json!({}), &PackageOptions::default()).unwrap();
    let path = artifact.path().to_owned();
    assert!(path.is_file());
    drop(artifact);
    assert!(!path.exists());
    assert!(!path.parent().unwrap().exists());
    let artifact = package_project(project.path(), &json!({}), &PackageOptions::default()).unwrap();
    let persisted = artifact.persist();
    assert_eq!(read_archive(&persisted).0, ["runpod_manifest.json"]);
    // Only remove the exact file and now-empty directory returned by this test's artifact.
    fs::remove_file(&persisted).unwrap();
    fs::remove_dir(persisted.parent().unwrap()).unwrap();
}

#[test]
fn binary_files_long_paths_and_hard_links_are_archived_as_independent_files() {
    let project = tempfile::tempdir().unwrap();
    let long_name = format!("{}.bin", "a".repeat(150));
    let bytes: Vec<_> = (0..=255).collect();
    fs::write(project.path().join(&long_name), &bytes).unwrap();
    fs::hard_link(
        project.path().join(&long_name),
        project.path().join("hardlink.bin"),
    )
    .unwrap();
    let artifact = package_project(project.path(), &json!({}), &PackageOptions::default()).unwrap();
    let (_, contents) = read_archive(artifact.path());
    assert_eq!(contents[&long_name], bytes);
    assert_eq!(contents["hardlink.bin"], bytes);
}

#[test]
fn ignore_read_and_output_write_failures_propagate() {
    let project = tempfile::tempdir().unwrap();
    fs::write(project.path().join(".gitignore"), [0xff]).unwrap();
    assert!(package_project(project.path(), &json!({}), &PackageOptions::default()).is_err());
    fs::remove_file(project.path().join(".gitignore")).unwrap();
    let options = PackageOptions {
        output: Some(project.path().join("missing/output.bin")),
        env_dir: None,
    };
    assert!(package_project(project.path(), &json!({}), &options).is_err());
}

#[cfg(unix)]
#[test]
fn test_symlinks_cannot_escape_source_or_dependencies() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let env = temp.path().join("built-env");
    let outside = temp.path().join("outside");
    for root in [&project, &env, &outside] {
        fs::create_dir(root).unwrap();
    }
    fs::write(project.join("main.py"), "source").unwrap();
    fs::set_permissions(project.join("main.py"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(outside.join("secret.txt"), "private").unwrap();
    fs::write(outside.join("ignore"), "main.py\n").unwrap();
    for root in [&project, &env] {
        symlink(outside.join("secret.txt"), root.join("linked.py")).unwrap();
        symlink(&outside, root.join("linked-dir")).unwrap();
        symlink(outside.join("missing"), root.join("broken-link")).unwrap();
    }
    symlink(outside.join("ignore"), project.join(".gitignore")).unwrap();
    symlink(outside.join("ignore"), project.join(".runpodignore")).unwrap();
    let artifact = package_project(
        &project,
        &json!({}),
        &PackageOptions {
            env_dir: Some(env),
            ..PackageOptions::default()
        },
    )
    .unwrap();
    assert_eq!(
        read_archive(artifact.path()).0,
        ["main.py", "runpod_manifest.json"]
    );
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(
        fs::File::open(artifact.path()).unwrap(),
    ));
    assert_eq!(
        archive
            .entries()
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .header()
            .mode()
            .unwrap()
            & 0o777,
        0o755
    );
}

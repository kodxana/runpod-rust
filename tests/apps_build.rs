use base64::{Engine as _, engine::general_purpose::STANDARD};
use runpod::{
    Value,
    apps::{
        ResourceSpec,
        build::{
            ExclusionOptions, collect_requirements, installed_closure, prune_excluded,
            requirement_name, split_exclusions,
        },
        deploy::{AppDefinition, ResourceDefinition},
    },
    json,
};
use std::{collections::BTreeMap, fs, path::Path};

fn snapshot(root: &Path) -> (BTreeMap<String, String>, Vec<String>) {
    let mut files = BTreeMap::new();
    let mut dirs = Vec::new();
    let mut pending = vec![root.to_owned()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let name = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if path.is_dir() {
                dirs.push(name);
                pending.push(path);
            } else {
                files.insert(name, STANDARD.encode(fs::read(path).unwrap()));
            }
        }
    }
    dirs.sort();
    (files, dirs)
}

fn python_cases(op: &str, prefix: &str) {
    let fixtures: Vec<Value> = serde_json::from_str(include_str!("fixtures/build.json")).unwrap();
    let fixtures: Vec<_> = fixtures
        .into_iter()
        .filter(|case| case["op"] == op && case["name"].as_str().unwrap().starts_with(prefix))
        .collect();
    assert!(!fixtures.is_empty(), "no fixtures for {op}/{prefix}");
    for case in fixtures {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let args = &case["args"];
        if let Some(files) = case["files"].as_object() {
            for (name, bytes) in files {
                let path = root.join(name);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, STANDARD.decode(bytes.as_str().unwrap()).unwrap()).unwrap();
            }
        }
        let output = match case["op"].as_str().unwrap() {
            "name" => json!(requirement_name(args.as_str().unwrap())),
            "split" => {
                let reqs: Vec<String> =
                    serde_json::from_value(args["requirements"].clone()).unwrap();
                let options: ExclusionOptions = serde_json::from_value(args.clone()).unwrap();
                json!(split_exclusions(&reqs, &options))
            }
            "collect" => {
                if let Some(text) = args["text"].as_str() {
                    fs::write(root.join("requirements.txt"), text).unwrap();
                }
                let mut app = AppDefinition::new("app", "default").unwrap();
                for deps in args["dependencies"].as_array().unwrap() {
                    app.resources.push(ResourceDefinition {
                        spec: ResourceSpec {
                            dependencies: Some(serde_json::from_value(deps.clone()).unwrap()),
                            ..ResourceSpec::default()
                        },
                        symbol: None,
                    });
                }
                json!(collect_requirements(root, &app).unwrap())
            }
            "closure" => json!(
                installed_closure(
                    root,
                    &serde_json::from_value::<Vec<String>>(args["roots"].clone()).unwrap()
                )
                .unwrap()
            ),
            "prune" => {
                let dropped = prune_excluded(
                    root,
                    &serde_json::from_value::<Vec<String>>(args["excluded"].clone()).unwrap(),
                    &serde_json::from_value::<Vec<String>>(args["roots"].clone()).unwrap(),
                    &serde_json::from_value::<Vec<String>>(args["image_provided"].clone()).unwrap(),
                )
                .unwrap();
                let (files, dirs) = snapshot(root);
                json!({"dropped":dropped,"files":files,"dirs":dirs})
            }
            other => panic!("unknown case {other}"),
        };
        assert_eq!(output, case["expected"], "{}", case["name"]);
    }
}

#[test]
fn pruning_does_not_reinterpret_absolute_records_as_local_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let info = root.join("doomed-1.dist-info");
    fs::create_dir(&info).unwrap();
    fs::write(info.join("METADATA"), "Name: doomed\n").unwrap();
    fs::write(info.join("RECORD"),"/keep.txt,,\n../outside.txt,,\n../../../outside.txt,,\nC:/keep.txt,,\n../../bin/installed,,\n\"data,a.bin\",,\n").unwrap();
    fs::create_dir(root.join("bin")).unwrap();
    fs::write(root.join("bin/installed"), "remove").unwrap();
    fs::write(root.join("data,a.bin"), "remove").unwrap();
    fs::write(root.join("keep.txt"), "keep").unwrap();
    assert_eq!(
        prune_excluded(root, &["doomed".into()], &[], &[]).unwrap(),
        ["doomed"]
    );
    assert!(root.join("keep.txt").is_file());
    assert!(!root.join("bin").exists());
    assert!(!root.join("data,a.bin").exists());
    assert!(!info.exists());
}

#[cfg(unix)]
#[test]
fn pruning_never_follows_file_directory_or_dist_info_links_outside_the_environment() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("env");
    let outside = temp.path().join("outside");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("keep.txt"), "keep").unwrap();
    let info = root.join("doomed-1.dist-info");
    fs::create_dir(&info).unwrap();
    fs::write(info.join("METADATA"), "Name: doomed\n").unwrap();
    fs::write(
        info.join("RECORD"),
        "linked-file,,\nlinked-dir/keep.txt,,\nbroken-link,,\n",
    )
    .unwrap();
    symlink(outside.join("keep.txt"), root.join("linked-file")).unwrap();
    symlink(&outside, root.join("linked-dir")).unwrap();
    symlink(root.join("missing"), root.join("broken-link")).unwrap();
    let other = outside.join("external-1.dist-info");
    fs::create_dir(&other).unwrap();
    fs::write(other.join("METADATA"), "Name: external\n").unwrap();
    symlink(&other, root.join("external-1.dist-info")).unwrap();
    let dropped = prune_excluded(&root, &["doomed".into(), "external".into()], &[], &[]).unwrap();
    assert_eq!(dropped, ["doomed", "external"]);
    assert_eq!(
        fs::read_to_string(outside.join("keep.txt")).unwrap(),
        "keep"
    );
    assert!(other.join("METADATA").is_file());
    assert!(root.join("external-1.dist-info").is_symlink());
    assert!(root.join("linked-file").is_symlink());
    assert!(root.join("linked-dir").is_symlink());
    assert!(!root.join("broken-link").is_symlink());
}

macro_rules! cases {
    ($($name:ident: $op:literal, $prefix:literal),+ $(,)?) => {$(
        #[test] fn $name() { python_cases($op, $prefix); }
    )+};
}

#[test]
fn test_exclusion_set_is_size_based() {
    let actual: std::collections::BTreeSet<_> = runpod::apps::build::SIZE_PROHIBITIVE_PACKAGES
        .iter()
        .copied()
        .collect();
    assert_eq!(
        actual,
        ["torch", "torchvision", "torchaudio", "triton"]
            .into_iter()
            .collect()
    );
}
cases! {
    test_extracts_distribution_name: "name", "",
    test_merges_requirements_txt_and_decorator_deps: "collect", "merges_requirements_txt_and_decorator_deps",
    test_no_requirements_file: "collect", "no_requirements_file",
    requirement_names_keep_the_first_pin: "collect", "first_names_win",
    test_torch_family_auto_excluded: "split", "torch_family_auto_excluded",
    test_user_excludes_added: "split", "user_excludes_added",
    test_auto_exclude_off_vendors_everything: "split", "auto_exclude_off_vendors_everything",
    test_auto_exclude_off_user_excludes_still_apply: "split", "auto_exclude_off_user_excludes_still_apply",
    exclusions_are_unique_and_sorted: "split", "deduplicated_sorted_exclusions",
    test_named_url_requirements_preserve_extras_and_markers: "closure", "named_url_requirements_preserve_extras_and_markers",
    test_unnamed_roots_fail_closed: "closure", "unnamed_roots_fail_closed",
    extra_markers_are_independent_of_builder_platform: "closure", "marker_",
    test_each_extra_is_a_separate_marker_context: "closure", "marker_5_",
    test_or_in_extra_name_is_not_an_operator: "closure", "marker_6_",
    test_transitive_extras_activate_grouped_marker: "closure", "transitive_extras_activate_grouped_marker",
    test_invalid_marker_fails_closed: "closure", "invalid_marker_fails_closed",
    missing_metadata_fails_closed: "closure", "missing_metadata_fails_closed",
    late_extra_activation_revisits_cycles: "closure", "cycle_with_late_extra",
    test_transitive_torch_and_its_tree_removed: "prune", "transitive_torch_and_its_tree_removed",
    test_root_requirement_is_kept: "prune", "root_requirement_is_kept",
    test_nothing_excluded_installed_is_noop: "prune", "nothing_excluded_installed_is_noop",
    test_missing_record_fails_closed: "prune", "missing_record_fails_closed",
    test_missing_record_on_excluded_falls_back_to_dir: "prune", "missing_record_on_excluded_falls_back_to_dir",
    test_unnamed_root_fails_closed: "prune", "unnamed_root_fails_closed",
    test_unreadable_requires_dist_fails_closed: "prune", "unreadable_requires_dist_fails_closed",
    test_user_exclude_keeps_its_dependencies: "prune", "user_exclude_keeps_its_dependencies",
    test_no_image_provided_removes_only_named: "prune", "no_image_provided_removes_only_named",
}

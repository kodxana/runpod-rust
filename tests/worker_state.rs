use runpod::{
    json,
    serverless::{JobsProgress, PING_MIRROR_CAPACITY, PingJobMirror, WorkerJob},
};

fn job(id: &str) -> WorkerJob {
    WorkerJob::new(id, json!(null))
}

#[test]
fn python_mirror_sequences_match() {
    let cases: Vec<runpod::Value> =
        serde_json::from_str(include_str!("fixtures/job_mirror.json")).unwrap();
    for case in cases {
        let mirror =
            PingJobMirror::with_capacity(case["capacity"].as_u64().unwrap().try_into().unwrap());
        for (input, expected) in case["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .zip(case["outputs"].as_array().unwrap())
        {
            mirror.set(input.as_str());
            assert_eq!(
                json!(mirror.get()),
                *expected,
                "capacity={} input={input}",
                case["capacity"]
            );
        }
    }
}
#[test]
fn test_set_get_roundtrip() {
    let mirror = PingJobMirror::new();
    mirror.set(Some("job_a,job_b"));
    assert_eq!(mirror.get().as_deref(), Some("job_a,job_b"));
}
#[test]
fn test_empty_is_none() {
    let mirror = PingJobMirror::new();
    assert_eq!(mirror.get(), None);
    mirror.set(None);
    assert_eq!(mirror.get(), None);
    mirror.set(Some(""));
    assert_eq!(mirror.get(), None);
}
#[test]
fn test_overflow_truncates_without_raising() {
    let mirror = PingJobMirror::with_capacity(32);
    mirror.set(Some(
        &(0..10)
            .map(|n| format!("id{n:08}"))
            .collect::<Vec<_>>()
            .join(","),
    ));
    assert_eq!(mirror.get().as_deref(), Some("id00000000,id00000001"));
}
#[test]
fn test_capacity_default() {
    assert_eq!(PING_MIRROR_CAPACITY, 65536);
    let mirror = PingJobMirror::new();
    mirror.set(Some(&"x".repeat(PING_MIRROR_CAPACITY + 1)));
    assert_eq!(mirror.get().unwrap().len(), PING_MIRROR_CAPACITY - 1);
}
#[test]
fn test_mirror_roundtrip_across_threads() {
    let mirror = PingJobMirror::new();
    mirror.set(Some("job_a,job_b"));
    let reader = mirror.clone();
    assert_eq!(
        std::thread::spawn(move || reader.get())
            .join()
            .unwrap()
            .as_deref(),
        Some("job_a,job_b")
    );
}
#[test]
fn test_set_mirror_pushes_current_state() {
    let jobs = JobsProgress::new();
    jobs.add(job("job_1"));
    let mirror = PingJobMirror::new();
    jobs.set_mirror(Some(mirror.clone()));
    assert_eq!(mirror.get().as_deref(), Some("job_1"));
}
#[test]
fn test_add_and_remove_sync_the_mirror() {
    let jobs = JobsProgress::new();
    let mirror = PingJobMirror::new();
    jobs.set_mirror(Some(mirror.clone()));
    jobs.add(job("job_1"));
    assert_eq!(mirror.get().as_deref(), Some("job_1"));
    jobs.remove("job_1");
    assert_eq!(mirror.get(), None);
}
#[test]
fn test_clear_syncs_the_mirror() {
    let jobs = JobsProgress::new();
    let mirror = PingJobMirror::new();
    jobs.set_mirror(Some(mirror.clone()));
    jobs.add(job("job_1"));
    jobs.clear();
    assert_eq!(mirror.get(), None);
}
#[test]
fn test_mutation_without_mirror_does_not_raise() {
    let jobs = JobsProgress::new();
    jobs.add(job("job_1"));
    jobs.remove("job_1");
    jobs.clear();
    assert!(jobs.is_empty());
}
#[test]
fn test_two_workers_do_not_share_jobs() {
    let a = JobsProgress::new();
    let b = JobsProgress::new();
    let ma = PingJobMirror::new();
    let mb = PingJobMirror::new();
    a.set_mirror(Some(ma.clone()));
    b.set_mirror(Some(mb.clone()));
    a.add(job("a_job"));
    assert_eq!(ma.get().as_deref(), Some("a_job"));
    assert!(b.is_empty());
    assert_eq!(mb.get(), None);
}
#[test]
fn test_singleton_identity() {
    // Explicit clones replace Python's process-global singleton.
    let jobs = JobsProgress::new();
    let clone = jobs.clone();
    jobs.add(job("job_1"));
    assert_eq!(clone.get_job_count(), 1);
    clone.clear();
    assert!(jobs.is_empty());
}
#[test]
fn test_add_remove_clear_and_list() {
    let jobs = JobsProgress::new();
    jobs.add(WorkerJob::new("job_1", json!({"a":1})));
    jobs.add(job("job_2"));
    assert_eq!(jobs.get_job_count(), 2);
    assert_eq!(jobs.get("job_1").unwrap().input, json!({"a":1}));
    assert_eq!(jobs.get_job_list().as_deref(), Some("job_1,job_2"));
    jobs.remove("job_1");
    assert_eq!(jobs.get_job_count(), 1);
    jobs.clear();
    assert_eq!(jobs.get_job_list(), None);
}
#[test]
fn duplicate_missing_and_detached_mutations_keep_source_semantics() {
    let jobs = JobsProgress::new();
    let mirror = PingJobMirror::new();
    jobs.set_mirror(Some(mirror.clone()));
    jobs.add(WorkerJob::new("a", json!(1)));
    mirror.set(Some("stale"));
    jobs.add(WorkerJob::new("a", json!(2)));
    assert_eq!(jobs.get("a").unwrap().input, json!(1));
    assert_eq!(mirror.get().as_deref(), Some("a"));
    mirror.set(Some("stale"));
    jobs.remove("missing");
    assert_eq!(mirror.get().as_deref(), Some("a"));
    jobs.set_mirror(None);
    jobs.clear();
    assert_eq!(mirror.get().as_deref(), Some("a"));
}
#[test]
fn concurrent_mutations_cannot_publish_an_older_snapshot_last() {
    let jobs = JobsProgress::new();
    let mirror = PingJobMirror::new();
    jobs.set_mirror(Some(mirror.clone()));
    std::thread::scope(|scope| {
        for thread in 0..8 {
            let jobs = &jobs;
            scope.spawn(move || {
                for n in 0..100 {
                    let id = format!("{thread}-{n}");
                    jobs.add(job(&id));
                    if n % 2 == 0 {
                        jobs.remove(&id);
                    }
                }
            });
        }
    });
    assert_eq!(jobs.get_job_count(), 400);
    assert_eq!(mirror.get(), jobs.get_job_list());
    jobs.clear();
    assert_eq!(mirror.get(), None);
}

#[test]
fn test_writes_nothing_to_disk() {
    let root = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "state_process_child", "--ignored"])
        .current_dir(root.path())
        .env("RUNPOD_TEST_STATE_CHILD", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
#[ignore = "child entry point exercised by test_writes_nothing_to_disk"]
fn state_process_child() {
    assert_eq!(std::env::var("RUNPOD_TEST_STATE_CHILD").as_deref(), Ok("1"));
    let jobs = JobsProgress::new();
    let mirror = PingJobMirror::new();
    jobs.set_mirror(Some(mirror.clone()));
    jobs.add(job("job_1"));
    jobs.remove("job_1");
    jobs.add(job("job_2"));
    assert_eq!(mirror.get().as_deref(), Some("job_2"));
}

#[test]
fn warning_subscriber_can_inspect_tracker_without_deadlocking() {
    struct Observer {
        jobs: JobsProgress,
        reported: std::sync::mpsc::Sender<usize>,
    }
    impl tracing::Subscriber for Observer {
        fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }
        fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
        fn event(&self, _: &tracing::Event<'_>) {
            self.reported.send(self.jobs.get_job_count()).unwrap();
        }
        fn enter(&self, _: &tracing::span::Id) {}
        fn exit(&self, _: &tracing::span::Id) {}
    }
    let jobs = JobsProgress::new();
    jobs.set_mirror(Some(PingJobMirror::with_capacity(2)));
    let (reported, received) = std::sync::mpsc::channel();
    let observed = jobs.clone();
    let worker = std::thread::spawn(move || {
        tracing::subscriber::with_default(
            Observer {
                jobs: observed,
                reported,
            },
            || jobs.add(job("long-id")),
        );
    });
    assert_eq!(
        received
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("logging subscriber blocked on tracker lock"),
        1
    );
    worker.join().unwrap();
}

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use crate::Backend;

fn overlapping_workspace_requests(already_indexed: bool, new_file: bool, fail_first_pass: bool) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Target.php"), "<?php class Target {}\n").unwrap();
    let backend = Backend::new_test_with_workspace(dir.path().to_path_buf(), Vec::new());
    if already_indexed && new_file {
        backend.ensure_workspace_indexed_for_request();
    }
    let usage = dir.path().join("Usage.php");
    std::fs::write(&usage, "<?php new Target();\n").unwrap();
    let usage_uri = crate::util::path_to_uri(&dunce::canonicalize(&usage).unwrap());
    if already_indexed && !new_file {
        backend.ensure_workspace_indexed_for_request();
    }
    let initial_passes = backend
        .workspace_index
        .completed_passes
        .load(Ordering::Acquire);
    let scans = Arc::new(AtomicUsize::new(0));
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    let owner = {
        let backend = backend.clone_for_blocking();
        let scans = Arc::clone(&scans);
        std::thread::spawn(move || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                backend.ensure_workspace_indexed_with_progress(Some(&|_, message| {
                    if message == "Preparing workspace index" {
                        scans.fetch_add(1, Ordering::Relaxed);
                        started_tx.send(()).unwrap();
                        release_rx
                            .lock()
                            .unwrap()
                            .recv_timeout(Duration::from_secs(10))
                            .unwrap();
                        assert!(!fail_first_pass, "simulated indexing failure");
                    }
                }));
            }))
        })
    };
    started_rx.recv_timeout(Duration::from_secs(10)).unwrap();

    let (waiting_tx, waiting_rx) = mpsc::channel();
    let waiters: Vec<_> = (0..20)
        .map(|_| {
            let backend = backend.clone_for_blocking();
            let scans = Arc::clone(&scans);
            let waiting_tx = waiting_tx.clone();
            std::thread::spawn(move || {
                let reported_wait = AtomicBool::new(false);
                let reports = Mutex::new(Vec::new());
                backend.ensure_workspace_indexed_with_progress(Some(&|percentage, message| {
                    if message.starts_with("Waiting for workspace index")
                        && !reported_wait.swap(true, Ordering::Relaxed)
                    {
                        waiting_tx.send(()).unwrap();
                    }
                    if message == "Preparing workspace index" {
                        scans.fetch_add(1, Ordering::Relaxed);
                    }
                    reports.lock().unwrap().push((percentage, message));
                }));
                assert_eq!(
                    reports.lock().unwrap().last(),
                    Some(&(100, "Workspace index ready".to_string()))
                );
                backend.find_class_references("Target", false)
            })
        })
        .collect();
    for _ in 0..waiters.len() {
        waiting_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    }
    release_tx.send(()).unwrap();
    assert_eq!(owner.join().unwrap().is_err(), fail_first_pass);
    for waiter in waiters {
        let references = waiter.join().unwrap();
        assert_eq!(references.len(), 1);
        assert_eq!(references[0].uri.as_str(), usage_uri);
    }
    assert_eq!(
        scans.load(Ordering::Relaxed),
        1 + usize::from(fail_first_pass)
    );
    assert_eq!(
        backend
            .workspace_index
            .completed_passes
            .load(Ordering::Acquire),
        initial_passes + 1
    );
    assert!(backend.workspace_index.status.lock().is_none());

    // A later request must still discover an unreported file, even just
    // after the shared pass. There is no time-based freshness window.
    std::fs::write(dir.path().join("Later.php"), "<?php new Target();\n").unwrap();
    backend.ensure_workspace_indexed_for_request();
    assert_eq!(backend.find_class_references("Target", false).len(), 2);
    assert_eq!(
        backend
            .workspace_index
            .completed_passes
            .load(Ordering::Acquire),
        initial_passes + 2
    );
}

#[test]
fn overlapping_workspace_requests_share_initial_index() {
    overlapping_workspace_requests(false, true, false);
}

#[test]
fn overlapping_workspace_requests_share_refresh() {
    overlapping_workspace_requests(true, true, false);
}

#[test]
fn overlapping_workspace_requests_share_unchanged_refresh() {
    overlapping_workspace_requests(true, false, false);
}

#[test]
fn overlapping_workspace_requests_retry_failed_refresh() {
    overlapping_workspace_requests(true, true, true);
}

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::Backend;
use crate::atom::atom;
use crate::cancellation::{AnalysisCancellation, Cancelled};
use crate::text_position::LineIndex;
use crate::type_engine::variable::forward_walk;

use super::receivers::{ReceiverWalk, TEST_RESOLVED_RECEIVERS};

const SOURCE: &str = "<?php
class Service { public function keep() {} public function save() {} }
class Other { public function save() {} }
function first(Service $s) { $s->keep(); }
function second(Service $s) {
    $service = $s;
    $marker = 1;
    $service->save();
}
function third(Service $s) { $s->save(); }
function unrelated(Other $s) { $s->save(); }
";

#[derive(Clone, Copy)]
enum StopAt {
    Scope,
    Statement,
    Receiver,
}

fn interrupted_warmup(reason: Cancelled, stop_at: StopAt) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("consumer.php");
    std::fs::write(&path, SOURCE).unwrap();
    let uri = crate::util::path_to_uri(&dunce::canonicalize(path).unwrap());
    let backend = Backend::new_test_with_workspace(dir.path().to_path_buf(), Vec::new());
    backend.ensure_workspace_indexed_for_request();
    let content = Arc::new(SOURCE.to_string());
    let map = backend.symbol_map_for(&uri).unwrap();
    let lines = LineIndex::new(&content);
    let previous = backend.resolve_member_receivers(
        &uri,
        &map,
        &content,
        map.source(&content).unwrap(),
        &|offset| lines.position(offset as usize),
        ReceiverWalk::Names(&[atom("keep")]),
    );
    assert!(previous.covers([atom("keep")]));
    assert!(!previous.covers([atom("save")]));
    forward_walk::reset_test_body_walks();
    TEST_RESOLVED_RECEIVERS.with(|count| count.set(0));

    let started = Instant::now();
    let deadline = started + Duration::from_secs(60);
    let shutdown = Arc::clone(&backend.shutdown_flag);
    let mut cancellation = AnalysisCancellation::new(Arc::clone(&shutdown), deadline);
    let statement_offset = SOURCE.find("$marker =").unwrap() as u32;
    cancellation.clock = Some(Box::new(move || {
        let reached = match stop_at {
            // Three class methods and `first` have been walked when the
            // builder enters `second`.
            StopAt::Scope => forward_walk::test_body_walks() >= 5,
            StopAt::Statement => forward_walk::DIAGNOSTIC_SCOPE.with(|cache| {
                cache
                    .borrow()
                    .as_ref()
                    .is_some_and(|map| map.contains_key(&statement_offset))
            }),
            StopAt::Receiver => TEST_RESOLVED_RECEIVERS.with(|count| count.get() >= 1),
        };
        if reached {
            if reason == Cancelled::Shutdown {
                shutdown.store(true, Ordering::Release);
            }
            deadline
        } else {
            started
        }
    }));
    let result = Arc::new(cancellation).run(|| backend.warm_member_receivers(&uri));
    assert_eq!(result, Err(reason));
    assert!(!forward_walk::is_diagnostic_scope_active());
    assert!(!forward_walk::is_building_scopes());
    assert!(!crate::type_engine::call_resolution::body_inference_in_progress());
    let retained = backend.resolved_member_file(&uri, &map).unwrap();
    assert!(
        Arc::ptr_eq(&previous, &retained),
        "an interrupted walk must leave complete earlier coverage intact"
    );
    backend.shutdown_flag.store(false, Ordering::Release);

    let declaration = lines.position(SOURCE.find("function save").unwrap() + "function ".len());
    let references = backend
        .find_references(&uri, SOURCE, declaration, false)
        .unwrap();
    let mut positions: Vec<_> = references
        .iter()
        .map(|location| {
            assert_eq!(location.uri.as_str(), uri);
            location.range.start
        })
        .collect();
    positions.sort_by_key(|position| (position.line, position.character));
    assert_eq!(
        positions,
        vec![
            lines.position(SOURCE.find("$service->save").unwrap() + "$service->".len()),
            lines.position(
                SOURCE.find("function third").unwrap() + "function third(Service $s) { $s->".len()
            ),
        ]
    );
}

#[test]
fn warmup_shutdown_between_scopes_keeps_references_complete() {
    interrupted_warmup(Cancelled::Shutdown, StopAt::Scope);
}

#[test]
fn warmup_deadline_between_scopes_keeps_references_complete() {
    interrupted_warmup(Cancelled::Deadline, StopAt::Scope);
}

#[test]
fn warmup_shutdown_inside_a_body_keeps_references_complete() {
    interrupted_warmup(Cancelled::Shutdown, StopAt::Statement);
}

#[test]
fn warmup_deadline_inside_a_body_keeps_references_complete() {
    interrupted_warmup(Cancelled::Deadline, StopAt::Statement);
}

#[test]
fn warmup_shutdown_during_receivers_keeps_references_complete() {
    interrupted_warmup(Cancelled::Shutdown, StopAt::Receiver);
}

#[test]
fn warmup_deadline_during_receivers_keeps_references_complete() {
    interrupted_warmup(Cancelled::Deadline, StopAt::Receiver);
}

#[test]
fn warmup_cancellation_does_not_memoize_an_incomplete_callee() {
    let source = "<?php
class Box { public function open(): void {} }
class Factory { public static function make() { return new Box(); } }
function run(): void {
    $first = Factory::make();
    $first->open();
    $second = Factory::make();
    $second->open();
}
";
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("consumer.php");
    std::fs::write(&path, source).unwrap();
    let uri = crate::util::path_to_uri(&dunce::canonicalize(path).unwrap());
    let backend = Backend::new_test_with_workspace(dir.path().to_path_buf(), Vec::new());
    backend.ensure_workspace_indexed_for_request();
    let map = backend.symbol_map_for(&uri).unwrap();
    let started = Instant::now();
    let deadline = started + Duration::from_secs(60);
    let mut cancellation = AnalysisCancellation::new(Arc::clone(&backend.shutdown_flag), deadline);
    cancellation.clock = Some(Box::new(move || {
        if crate::type_engine::call_resolution::body_inference_in_progress() {
            deadline
        } else {
            started
        }
    }));
    // Keep the memos alive across cancellation and retry: even a caller
    // retaining them must never read a partial return type as a result.
    let _caches = crate::type_engine::call_resolution::activate_type_engine_caches();
    let before = crate::type_engine::call_resolution::test_body_inferences();
    assert_eq!(
        Arc::new(cancellation).run(|| backend.warm_member_receivers(&uri)),
        Err(Cancelled::Deadline)
    );
    assert!(backend.resolved_member_file(&uri, &map).is_none());
    assert!(!forward_walk::is_diagnostic_scope_active());
    assert!(!forward_walk::is_building_scopes());
    assert!(!crate::type_engine::call_resolution::body_inference_in_progress());
    assert!(backend.warm_member_receivers(&uri));
    assert_eq!(
        crate::type_engine::call_resolution::test_body_inferences() - before,
        2
    );
    let position = LineIndex::new(source).position(source.find("open()").unwrap());
    assert_eq!(
        backend
            .find_references(&uri, source, position, false)
            .unwrap()
            .len(),
        2
    );
}

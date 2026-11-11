use super::*;
use std::time::Duration;

#[test]
fn cancellation_distinguishes_shutdown_and_deadline() {
    let shutdown = Arc::new(AtomicBool::new(false));
    let expired = Arc::new(AnalysisCancellation::new(
        Arc::clone(&shutdown),
        Instant::now(),
    ));
    assert_eq!(
        expired.run(|| unreachable!()),
        Err::<(), _>(Cancelled::Deadline)
    );
    shutdown.store(true, Ordering::Release);
    let stopped = Arc::new(AnalysisCancellation::new(
        shutdown,
        Instant::now() + Duration::from_secs(60),
    ));
    assert_eq!(
        stopped.run(|| unreachable!()),
        Err::<(), _>(Cancelled::Shutdown)
    );
    checkpoint();
}

#[test]
fn cancellation_restores_outer_operation_and_preserves_real_panics() {
    let shutdown = Arc::new(AtomicBool::new(false));
    let outer = Arc::new(AnalysisCancellation::new(
        Arc::clone(&shutdown),
        Instant::now() + Duration::from_secs(60),
    ));
    let expired = Arc::new(AnalysisCancellation::new(
        Arc::clone(&shutdown),
        Instant::now(),
    ));
    assert_eq!(
        outer.run(|| {
            assert_eq!(expired.run(|| 1), Err(Cancelled::Deadline));
            checkpoint();
            shutdown.store(true, Ordering::Release);
            checkpoint();
            2
        }),
        Err(Cancelled::Shutdown)
    );
    checkpoint();
    shutdown.store(false, Ordering::Release);
    let panic = catch_unwind(AssertUnwindSafe(|| {
        outer.run(|| panic!("ordinary failure"))
    }));
    assert_eq!(
        panic.unwrap_err().downcast_ref::<&str>(),
        Some(&"ordinary failure")
    );
    checkpoint();
}

#[test]
fn cancellation_crosses_cached_and_uncached_parser_recovery() {
    let content = Arc::new("<?php function work() {}".to_string());
    for cached in [false, true] {
        let shutdown = Arc::new(AtomicBool::new(false));
        let operation = Arc::new(AnalysisCancellation::new(
            Arc::clone(&shutdown),
            Instant::now() + Duration::from_secs(60),
        ));
        let result = operation.run(|| {
            let _cache = cached.then(|| crate::parser::with_parse_cache_arc(Arc::clone(&content)));
            crate::util::catch_panic_unwind_safe("cancellation test", "", None, || {
                crate::parser::with_parsed_program(&content, "cancellation test", |_, _| {
                    shutdown.store(true, Ordering::Release);
                    checkpoint();
                    42
                })
            })
        });
        assert_eq!(result, Err(Cancelled::Shutdown));
        checkpoint();
    }
}

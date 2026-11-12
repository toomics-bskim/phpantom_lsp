//! A bounded producer for work whose publication must remain ordered.

/// Prepare the next item while the caller publishes the previous one.
/// A rendezvous channel keeps at most two items alive: one being published
/// and one being prepared or waiting to be handed over. Publication finishes
/// before receiving the next item, including releasing its temporary state.
pub(crate) fn for_each_pipelined<T: Sync, R: Send>(
    items: &[T],
    prepare: impl Fn(&T) -> R + Sync,
    mut publish: impl FnMut(&T, R),
) {
    if items.len() <= 1 {
        for item in items {
            publish(item, prepare(item));
        }
        return;
    }

    std::thread::scope(|scope| {
        // The receiver must drop before the scope joins the producer when
        // publication unwinds, so a blocked send cannot deadlock the join.
        let (sender, receiver) = std::sync::mpsc::sync_channel(0);
        let prepare = &prepare;
        let producer = std::thread::Builder::new()
            .name("workspace-prepare".into())
            .stack_size(crate::PARSE_WORKER_STACK_SIZE)
            .spawn_scoped(scope, move || {
                for (index, item) in items.iter().enumerate() {
                    if sender.send((index, prepare(item))).is_err() {
                        break;
                    }
                }
            });
        let mut published = 0;
        match producer {
            Ok(producer) => {
                for (index, result) in receiver {
                    publish(&items[index], result);
                    published = index + 1;
                }
                if producer.join().is_err() {
                    tracing::error!(
                        "workspace preparation thread panicked; retrying remaining batches"
                    );
                }
            }
            Err(error) => tracing::error!("failed to spawn workspace preparation thread: {error}"),
        }
        for item in &items[published..] {
            publish(item, prepare(item));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn pipeline_overlaps_work_in_order_with_only_two_live_items() {
        struct Prepared<'a>(&'a AtomicUsize);
        impl Drop for Prepared<'_> {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::SeqCst);
            }
        }
        let live = AtomicUsize::new(0);
        let (ready, second_ready) = std::sync::mpsc::channel();
        let mut published = Vec::new();
        for_each_pipelined(
            &[0, 1, 2, 3],
            |index| {
                assert!(live.fetch_add(1, Ordering::SeqCst) < 2);
                if *index == 1 {
                    ready.send(()).unwrap();
                }
                Prepared(&live)
            },
            |index, result| {
                if *index == 0 {
                    second_ready.recv_timeout(Duration::from_secs(5)).unwrap();
                }
                published.push(*index);
                drop(result);
            },
        );
        assert_eq!(published, [0, 1, 2, 3]);
        assert_eq!(live.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn pipeline_recovers_unpublished_work_after_a_producer_panic() {
        let attempts = AtomicUsize::new(0);
        let mut published = Vec::new();
        for_each_pipelined(
            &[0, 1, 2],
            |index| {
                if *index == 1 && attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                    panic!("producer failed once");
                }
                *index
            },
            |_, result| published.push(result),
        );
        assert_eq!(published, [0, 1, 2]);
    }

    #[test]
    fn pipeline_releases_a_blocked_producer_when_publication_panics() {
        let result = std::panic::catch_unwind(|| {
            for_each_pipelined(&[0, 1, 2], |index| *index, |_, _| panic!("publish failed"));
        });
        assert!(result.is_err());
    }
}

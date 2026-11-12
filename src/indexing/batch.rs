//! Parallel parsing and bounded, ordered publication of workspace files.

use crate::Backend;
use crate::parser::AstIndexParseResult;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

impl Backend {
    /// Parse a batch of files in parallel using OS threads.
    ///
    /// Each entry is `(uri, optional_content)`.  When `content` is `None`,
    /// the file is loaded via [`get_file_content`].  Workers parse files into
    /// owned index updates, then a single merge publishes the whole batch.
    ///
    /// Runs on [`crate::parallel::map_indexed`], so every worker has
    /// finished before this method returns and the thread count is capped
    /// at the number of available CPU cores.
    pub(crate) fn parse_files_parallel_with_progress(
        &self,
        files: Vec<(String, Option<String>)>,
        progress: Option<&(dyn Fn(usize, usize, u64, u64) + Sync)>,
    ) {
        self.parse_batch_parallel(
            &files,
            |(uri, _)| uri.as_str(),
            |(uri, content)| self.index_progress_weight_for_uri(uri, content.as_deref()),
            |(uri, content)| content.clone().or_else(|| self.get_file_content(uri)),
            progress,
            true,
        );
    }

    /// Parse a batch of files from disk paths in parallel.
    ///
    /// Each entry is `(uri, path)`.  The file is read from disk and parsed in
    /// a worker thread.  Work is pulled from a shared atomic counter so large
    /// files cannot leave one fixed chunk as the long tail.
    pub(crate) fn parse_paths_parallel_with_progress(
        &self,
        files: &[(String, PathBuf)],
        progress: Option<&(dyn Fn(usize, usize, u64, u64) + Sync)>,
    ) {
        self.parse_batch_parallel(
            files,
            |(uri, _)| uri.as_str(),
            |(_, path)| index_progress_weight_for_path(path),
            |(_, path)| crate::util::read_php_source(path),
            progress,
            true,
        );
    }

    /// [`Self::parse_paths_parallel_with_progress`] for files a search is
    /// about to read, which keeps every one of them resident.
    pub(crate) fn parse_paths_parallel_awake(&self, files: &[(String, PathBuf)]) {
        self.parse_batch_parallel(
            files,
            |(uri, _)| uri.as_str(),
            |(_, path)| index_progress_weight_for_path(path),
            |(_, path)| crate::util::read_php_source(path),
            None,
            false,
        );
    }

    /// Parse a batch of items into owned index updates on parallel
    /// workers, then publish them.
    ///
    /// The caller supplies how to read an item's URI, its progress weight,
    /// and its content.  Everything else (work ordering, thread sizing,
    /// progress accounting, and the merge) is shared by every batch
    /// parser.  Workers claim indices from a shared cursor in
    /// largest-first order so one oversized file cannot become the long
    /// tail of the batch.
    ///
    /// The batch is published in a single merge, except for the files of
    /// installed packages when `let_sleep` is set: those are published
    /// [`DORMANT_CHUNK`] at a time and put to sleep (see
    /// [`Self::put_to_sleep`]) after each, so their parses never all sit
    /// in memory at once. The next chunk parses while the current one merges,
    /// with at most two chunks live.
    fn parse_batch_parallel<T: Sync>(
        &self,
        items: &[T],
        uri_of: impl Fn(&T) -> &str + Sync,
        weight_of: impl Fn(&T) -> u64 + Sync,
        content_of: impl Fn(&T) -> Option<String> + Sync,
        progress: Option<&(dyn Fn(usize, usize, u64, u64) + Sync)>,
        let_sleep: bool,
    ) {
        if items.is_empty() {
            return;
        }
        let weights: Vec<u64> = items.iter().map(&weight_of).collect();
        let counter = ParseCounter {
            parsed: AtomicUsize::new(0),
            parsed_units: AtomicU64::new(0),
            total: items.len(),
            total_units: weights.iter().copied().sum::<u64>().max(1),
        };
        let (dormant, resident): (Vec<usize>, Vec<usize>) = (0..items.len())
            .partition(|&idx| let_sleep && self.is_installed_package_file(uri_of(&items[idx])));

        let parse = |batch: &[usize]| {
            self.parse_batch(batch, &weights, &counter, progress, |idx| {
                let item = &items[idx];
                content_of(item)
                    .map(|content| self.parse_ast_index_update_for_index(uri_of(item), &content))
            })
        };
        self.publish_parsed(parse(&resident), &counter, progress);
        let chunks: Vec<_> = dormant.chunks(DORMANT_CHUNK).collect();
        crate::parallel::for_each_pipelined(
            &chunks,
            |chunk| parse(chunk),
            |chunk, results| {
                self.publish_parsed(results, &counter, progress);
                self.put_to_sleep(chunk.iter().map(|&idx| uri_of(&items[idx])));
            },
        );
    }

    /// Parse the items at `batch` (indices into `weights`) on parallel
    /// workers, retaining input order for publication.
    fn parse_batch(
        &self,
        batch: &[usize],
        weights: &[u64],
        counter: &ParseCounter,
        progress: Option<&(dyn Fn(usize, usize, u64, u64) + Sync)>,
        parse_item: impl Fn(usize) -> Option<AstIndexParseResult> + Sync,
    ) -> Vec<AstIndexParseResult> {
        if batch.is_empty() {
            return Vec::new();
        }
        let report = |idx: usize| {
            report_weighted_parse_progress(
                progress,
                &counter.parsed,
                &counter.parsed_units,
                weights[idx],
                counter.total,
                counter.total_units,
            );
        };

        // For very small batches, avoid thread overhead.
        if batch.len() <= 2 {
            let mut results = Vec::with_capacity(batch.len());
            for &idx in batch {
                results.extend(parse_item(idx));
                report(idx);
            }
            return results;
        }

        // Workers claim positions in the largest-first order, so the index
        // the pool reports is a position in that order, not the item's own.
        let batch_weights: Vec<u64> = batch.iter().map(|&idx| weights[idx]).collect();
        let work_order = largest_first_work_order(&batch_weights);
        let mut results =
            crate::parallel::map_indexed("workspace-parse", batch.len(), |_, work_idx| {
                let idx = batch[work_order[work_idx]];
                let result = parse_item(idx).map(|result| (idx, result));
                report(idx);
                result
            });
        results.sort_by_key(|(_, (idx, _))| *idx);
        results.into_iter().map(|(_, (_, result))| result).collect()
    }

    /// Publish one prepared batch, reporting the
    /// batch complete first when it is the last one.
    fn publish_parsed(
        &self,
        results: Vec<AstIndexParseResult>,
        counter: &ParseCounter,
        progress: Option<&(dyn Fn(usize, usize, u64, u64) + Sync)>,
    ) {
        if counter.parsed.load(Ordering::Relaxed) == counter.total {
            report_weighted_merge_progress(progress, counter.total, counter.total_units);
        }
        self.apply_ast_index_parse_results_batch(results);
    }

    pub(crate) fn index_progress_weight_for_uri(&self, uri: &str, content: Option<&str>) -> u64 {
        if let Some(content) = content {
            return (content.len() as u64).max(1);
        }
        if let Some(content) = self.open_files.read().get(uri) {
            return (content.len() as u64).max(1);
        }
        crate::util::uri_to_path(uri)
            .map(|path| index_progress_weight_for_path(&path))
            .unwrap_or(1)
    }
}

pub(crate) fn workspace_parse_percentage(done: u64, total: u64) -> u32 {
    if total == 0 {
        return 95;
    }

    5 + ((done.saturating_mul(90) / total).min(90) as u32)
}

fn report_weighted_parse_progress(
    progress: Option<&(dyn Fn(usize, usize, u64, u64) + Sync)>,
    parsed: &AtomicUsize,
    parsed_units: &AtomicU64,
    weight: u64,
    total: usize,
    total_units: u64,
) {
    let done = parsed.fetch_add(1, Ordering::Relaxed) + 1;
    let done_units = parsed_units.fetch_add(weight, Ordering::Relaxed) + weight;
    let file_report_every = (total / 100).max(1);
    let unit_report_every = (total_units / 100).max(1);
    let crossed_unit_boundary =
        done_units == total_units || done_units % unit_report_every < weight.min(unit_report_every);

    if done == 1 || done == total || done.is_multiple_of(file_report_every) || crossed_unit_boundary
    {
        report_weighted_progress(progress, done, total, done_units, total_units);
    }
}

fn report_weighted_merge_progress(
    progress: Option<&(dyn Fn(usize, usize, u64, u64) + Sync)>,
    total: usize,
    total_units: u64,
) {
    report_weighted_progress(progress, total, total, total_units, total_units);
}

fn report_weighted_progress(
    progress: Option<&(dyn Fn(usize, usize, u64, u64) + Sync)>,
    done: usize,
    total: usize,
    done_units: u64,
    total_units: u64,
) {
    if let Some(progress) = progress {
        progress(done, total, done_units, total_units);
    }
}

/// How many files of installed packages one merge publishes before they
/// are put to sleep, which bounds how many of their parses are in memory
/// at once.
const DORMANT_CHUNK: usize = 1024;

/// Progress shared by every merge of one [`Backend::parse_batch_parallel`]
/// call.
struct ParseCounter {
    parsed: AtomicUsize,
    parsed_units: AtomicU64,
    total: usize,
    total_units: u64,
}

pub(crate) fn largest_first_work_order(weights: &[u64]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..weights.len()).collect();
    order.sort_by_key(|&idx| std::cmp::Reverse(weights[idx]));
    order
}

pub(super) fn index_progress_weight_for_path(path: &Path) -> u64 {
    path.metadata().map(|meta| meta.len()).unwrap_or(1).max(1)
}

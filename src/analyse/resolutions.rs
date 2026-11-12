//! Stable reference targets for comparing two analyses of the same sources.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::Backend;
use crate::semantic_export::{OccurrenceKind, reference_occurrences};
use crate::text_position::{LineIndex, utf16_col_to_byte_offset};

#[derive(Serialize)]
struct Resolution<'a> {
    version: u8,
    path: &'a str,
    start: u32,
    end: u32,
    line: u32,
    column: u32,
    kind: OccurrenceKind,
    name: &'a str,
    targets: BTreeSet<String>,
}

/// Publish only a complete dump: a read or write failure leaves the previous
/// snapshot intact, so a truncated run cannot look like lost resolution.
pub(super) fn write_dump(
    destination: &Path,
    backend: Option<&Backend>,
    root: &Path,
    files: &[PathBuf],
    file_data: &[Option<(String, String)>],
) -> io::Result<()> {
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    let mut writer = BufWriter::new(temporary.as_file_mut());
    let mut ordered: Vec<_> = files
        .iter()
        .zip(file_data)
        .map(|(path, data)| {
            (
                path.strip_prefix(root)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .replace(std::path::MAIN_SEPARATOR, "/"),
                data,
            )
        })
        .collect();
    ordered.sort_by(|left, right| left.0.cmp(&right.0));
    let mut total = 0;
    let mut resolved = 0;
    let mut members = 0;
    let mut resolved_members = 0;
    for (path, data) in ordered {
        let (uri, original) = data
            .as_ref()
            .ok_or_else(|| io::Error::other(format!("could not read {path}")))?;
        let backend = backend.ok_or_else(|| io::Error::other("missing workspace index"))?;
        let source = backend.analysable_content_or(uri, original);
        let lines = LineIndex::new(&source);
        let original_lines = LineIndex::new(original);
        let template = backend.is_php_template(uri);
        let mut accesses = BTreeMap::<_, BTreeSet<String>>::new();
        for occurrence in reference_occurrences(backend, uri, &source) {
            let (start, end) = if template {
                let Some(range) = backend.try_translate_template_range(
                    uri,
                    tower_lsp_server::gen_lsp_types::Range {
                        start: lines.position(occurrence.range.start as usize),
                        end: lines.position(occurrence.range.end as usize),
                    },
                ) else {
                    continue;
                };
                let offset = |position: tower_lsp_server::gen_lsp_types::Position| {
                    let start = original_lines.line_start(position.line as usize);
                    let end = original_lines.line_start(position.line as usize + 1);
                    (start + utf16_col_to_byte_offset(&original[start..end], position.character))
                        as u32
                };
                (offset(range.start), offset(range.end))
            } else {
                (occurrence.range.start, occurrence.range.end)
            };
            // Compiler-inserted helpers can map to empty or clamped ranges.
            // Only copied tokens describe references in the user's template.
            if template
                && (start >= end
                    || original.get(start as usize..end as usize)
                        != source
                            .get(occurrence.range.start as usize..occurrence.range.end as usize))
            {
                continue;
            }
            let targets = accesses
                .entry((start, end, occurrence.kind, occurrence.name))
                .or_default();
            targets.extend(occurrence.resolved_symbol);
        }
        for ((start, end, kind, name), targets) in accesses {
            total += 1;
            resolved += usize::from(!targets.is_empty());
            if matches!(
                kind,
                OccurrenceKind::Method | OccurrenceKind::Property | OccurrenceKind::Constant
            ) {
                members += 1;
                resolved_members += usize::from(!targets.is_empty());
            }
            let position = original_lines.position(start as usize);
            serde_json::to_writer(
                &mut writer,
                &Resolution {
                    version: 1,
                    path: &path,
                    start,
                    end,
                    line: position.line + 1,
                    column: position.character,
                    kind,
                    name: &name,
                    targets,
                },
            )?;
            writer.write_all(b"\n")?;
        }
    }
    writer.flush()?;
    drop(writer);
    temporary
        .persist(destination)
        .map_err(|error| error.error)?;
    eprintln!(
        "Resolution coverage: {resolved}/{total} references; {resolved_members}/{members} member accesses"
    );
    Ok(())
}

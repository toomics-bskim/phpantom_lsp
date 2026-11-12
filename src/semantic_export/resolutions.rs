//! Reference-only export from an already indexed workspace.

use crate::Backend;
use crate::symbol_map::SymbolKind;
use crate::type_engine::resolver::LendsLoaders;

use super::ExportOccurrence;
use super::occurrences::{SpanExportContext, SpanExportOutput, export_span};

pub(crate) fn reference_occurrences(
    backend: &Backend,
    uri: &str,
    source: &str,
) -> Vec<ExportOccurrence> {
    let _project = backend.enter_project_scope(uri);
    let _version = crate::parser::SourceVersionGuard::new(backend.source_php_version(uri));
    let _parse = crate::parser::with_parse_cache(source);
    let _classes =
        crate::virtual_members::with_active_resolved_class_cache(&backend.resolved_class_cache);
    let _chains = crate::type_engine::resolver::with_chain_resolution_cache();
    let _types = crate::type_engine::call_resolution::activate_type_engine_caches();
    let _scopes = crate::type_engine::variable::forward_walk::with_diagnostic_scope_cache();
    let context = backend.file_context(uri);
    let Some(map) = backend.symbol_map_for(uri) else {
        return Vec::new();
    };
    let Some(mapped_source) = map.source(source) else {
        return Vec::new();
    };

    let class_loaders = backend.resolved_class_loaders(&context);
    let source_loaders = backend.class_loaders(&context);
    let loaders = backend.diagnostic_loaders(&context);
    crate::type_engine::variable::forward_walk::build_diagnostic_scopes(
        source,
        &context.classes,
        &class_loaders.as_dyn_resolved(),
        &source_loaders.as_dyn(),
        &context.block_imports(),
        Some(backend),
        Some(uri),
        loaders.loaders(),
        Some(&backend.resolved_class_cache),
    );
    let export_context = SpanExportContext {
        backend,
        source,
        mapped_source,
        file_context: &context,
        variable_definitions: &[],
        variable_definition_offsets: None,
    };
    let mut output = SpanExportOutput::default();
    for span in &map.spans {
        if matches!(
            span.kind,
            SymbolKind::ClassReference { .. }
                | SymbolKind::SelfStaticParent(_)
                | SymbolKind::MemberAccess { .. }
                | SymbolKind::FunctionCall {
                    is_definition: false,
                    is_docblock_reference: false,
                    ..
                }
        ) {
            export_span(&export_context, span, &mut output);
            output.diagnostics.clear();
        }
    }
    output.occurrences
}

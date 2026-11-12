//! Constant reads that have no declaration, with PHP's legacy string fallback.

use mago_span::{HasSpan, Span};
use mago_syntax::cst::*;
use tower_lsp_server::gen_lsp_types::*;

use super::helpers::{FileDiagnosticContext, make_diagnostic};
use crate::Backend;

impl Backend {
    pub(crate) fn collect_unknown_constant_diagnostics_with_context(
        &self,
        ctx: &FileDiagnosticContext,
        uri: &str,
        content: &str,
        out: &mut Vec<Diagnostic>,
    ) {
        let reads = crate::parser::with_parsed_program(content, "constant_reads", |program, _| {
            let mut reads = Vec::new();
            for statement in program.statements.iter() {
                mago_syntax::walker::Walker::walk_statement(&ConstantReads, statement, &mut reads);
            }
            reads
        });
        for span in reads {
            let offset = span.start.offset;
            if !super::incremental::reports_site(offset) {
                continue;
            }
            let Some(name) = content.get(offset as usize..span.end.offset as usize) else {
                continue;
            };
            if ["true", "false", "null"]
                .iter()
                .any(|literal| name.eq_ignore_ascii_case(literal))
            {
                continue;
            }
            if self.constant_is_declared_at(name, offset, &ctx.file)
                || self.constant_is_guarded(uri, content, name, offset)
            {
                continue;
            }
            let fallback = self.undefined_constant_type(name, Some(uri), content, offset);
            let (severity, message) = if let Some(value) = fallback {
                let severity = if self.php_version() < crate::types::PhpVersion::new(7, 2) {
                    DiagnosticSeverity::Information
                } else {
                    DiagnosticSeverity::Warning
                };
                (
                    severity,
                    format!(
                        "Undefined constant {name}; PHP {} uses the string {value}",
                        self.php_version()
                    ),
                )
            } else {
                (
                    DiagnosticSeverity::Error,
                    format!("Undefined constant {name}"),
                )
            };
            if let Some(range) = self.offset_range_to_lsp_range(
                uri,
                content,
                offset as usize,
                span.end.offset as usize,
            ) {
                out.push(make_diagnostic(
                    range,
                    severity,
                    "unknown_constant",
                    message,
                ));
            }
        }
    }
}

struct ConstantReads;
impl<'ast, 'arena> mago_syntax::walker::Walker<'ast, 'arena, Vec<Span>> for ConstantReads {
    fn walk_in_constant_access(&self, node: &'ast ConstantAccess<'arena>, reads: &mut Vec<Span>) {
        reads.push(node.name.span());
    }
}

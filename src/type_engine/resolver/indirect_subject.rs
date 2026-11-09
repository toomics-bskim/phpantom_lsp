//! Resolve indirect subjects using the same CST and RHS reader as assignments.

use mago_span::HasSpan;
use mago_syntax::cst::Node;

use super::{Loaders, ResolutionCtx, VarResolutionCtx};
use crate::types::{AccessKind, ResolvedType};

pub(super) fn resolve(
    text: &str,
    access: AccessKind,
    ctx: &ResolutionCtx<'_>,
) -> Vec<ResolvedType> {
    let content = ctx.file.content;
    let limit = (ctx.cursor_offset as usize).min(content.len());
    let start = if content
        .get(limit..)
        .is_some_and(|tail| tail.starts_with(text))
    {
        Some(limit)
    } else {
        content.get(..limit).and_then(|prefix| prefix.rfind(text))
    };
    let Some(start) = start else {
        return Vec::new();
    };
    let end = start + text.len();
    let placeholder;
    let class = match ctx.current_class {
        Some(class) => class,
        None => {
            placeholder =
                crate::class_lookup::class_context_placeholder(content, ctx.cursor_offset);
            &placeholder
        }
    };
    let var_ctx = VarResolutionCtx {
        loaders: Loaders::with_function(ctx.function_loader),
        resolved_class_cache: ctx.resolved_class_cache,
        scope_var_resolver: ctx.scope_var_resolver,
        ..VarResolutionCtx::new("", ctx.file, class, ctx.cursor_offset)
    };
    let result = crate::parser::with_parsed_program(content, "indirect_subject", |program, _| {
        // Descend only through nodes enclosing the requested source range.
        // In particular, never re-parse the name as modern PHP or change the
        // offsets used to read arguments and imports from the original file.
        let mut stack = vec![Node::Program(program)];
        while let Some(node) = stack.pop() {
            let span = node.span();
            if span.start.offset as usize > start || (span.end.offset as usize) < end {
                continue;
            }
            if let Node::Expression(expression) = node
                && span.start.offset as usize == start
                && span.end.offset as usize == end
            {
                return crate::type_engine::variable::rhs_resolution::resolve_rhs_expression(
                    expression, &var_ctx,
                );
            }
            node.visit_children(|child| {
                let span = child.span();
                if span.start.offset as usize <= start && span.end.offset as usize >= end {
                    stack.push(child);
                }
            });
        }
        Vec::new()
    });
    if access == AccessKind::DoubleColon {
        let mut classes = Vec::new();
        for value in &result {
            for class in
                crate::type_engine::variable::class_string_resolution::classes_named_by_type(
                    &value.type_string,
                    &class.fqn(),
                    ctx.file.all_classes,
                    ctx.file.class_loader,
                )
            {
                ResolvedType::push_unique(&mut classes, ResolvedType::from_arc(class));
            }
        }
        if !classes.is_empty() {
            return classes;
        }
    }
    result
}

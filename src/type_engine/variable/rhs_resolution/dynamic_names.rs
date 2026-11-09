//! Resolve finite runtime names through the ordinary expression and scope readers.

use std::cell::Cell;

use mago_syntax::cst::{ClassLikeMemberSelector, Variable};

use crate::atom::bytes_to_str;
use crate::php_type::PhpType;
use crate::type_engine::resolver::VarResolutionCtx;
use crate::types::ResolvedType;

use super::{resolve_rhs_expression, resolve_var_types};

const MAX_NAMES: usize = 16;
const MAX_INDIRECTIONS: usize = 32;

thread_local! {
    static DEPTH: Cell<usize> = const { Cell::new(0) };
}

struct NameReadGuard;

impl NameReadGuard {
    fn enter() -> Option<Self> {
        let depth = DEPTH.get();
        if depth >= MAX_INDIRECTIONS {
            return None;
        }
        DEPTH.set(depth + 1);
        Some(Self)
    }
}

impl Drop for NameReadGuard {
    fn drop(&mut self) {
        DEPTH.set(DEPTH.get() - 1);
    }
}

/// Read a variable, including an indirect name, using the current walker's scope.
pub(super) fn variable_value(
    variable: &Variable<'_>,
    ctx: &VarResolutionCtx<'_>,
) -> Vec<ResolvedType> {
    if let Variable::Direct(variable) = variable {
        return resolve_var_types(bytes_to_str(variable.name), ctx, ctx.cursor_offset);
    }
    let Some(_guard) = NameReadGuard::enter() else {
        return unknown();
    };
    let Some(names) = variable_names(variable, ctx) else {
        return unknown();
    };
    let mut result = Vec::new();
    for name in names {
        let value = resolve_var_types(&format!("${name}"), ctx, ctx.cursor_offset);
        if value.is_empty() {
            // One unresolved candidate is still a possible runtime value.
            return unknown();
        }
        result.extend(value);
    }
    result
}

/// Names of the slots a variable denotes, without reading those slots.
/// For a static property this also distinguishes `Cls::$name` (a literal
/// property name) from `Cls::$$name` and `Cls::${$name}`.
pub(crate) fn variable_names(
    variable: &Variable<'_>,
    ctx: &VarResolutionCtx<'_>,
) -> Option<Vec<String>> {
    match variable {
        Variable::Direct(variable) => Some(vec![
            bytes_to_str(variable.name).strip_prefix('$')?.to_owned(),
        ]),
        Variable::Indirect(variable) => {
            known_names(&resolve_rhs_expression(variable.expression, ctx))
        }
        Variable::Nested(variable) => known_names(&variable_value(variable.variable, ctx)),
    }
}

/// Resolve a member selector to bounded literal names. An unknown alternative
/// prevents a precise lookup instead of silently dropping that possibility.
pub(super) fn member_names(
    selector: &ClassLikeMemberSelector<'_>,
    ctx: &VarResolutionCtx<'_>,
) -> Option<Vec<String>> {
    let _guard = NameReadGuard::enter()?;
    match selector {
        ClassLikeMemberSelector::Identifier(ident) => {
            Some(vec![bytes_to_str(ident.value).to_owned()])
        }
        ClassLikeMemberSelector::Variable(variable) => known_names(&variable_value(variable, ctx)),
        ClassLikeMemberSelector::Expression(selector) => {
            known_names(&resolve_rhs_expression(selector.expression, ctx))
        }
        ClassLikeMemberSelector::Missing(_) => None,
    }
}

/// The bounded literal strings a set of resolved values can name.
pub(in crate::type_engine) fn known_names(types: &[ResolvedType]) -> Option<Vec<String>> {
    if types.is_empty() {
        return None;
    }
    let mut names = Vec::new();
    for ty in types {
        for member in ty.type_string.union_members() {
            let value = member.as_literal()?.string_content()?;
            if !names.iter().any(|name| name == value.as_ref()) {
                if names.len() == MAX_NAMES {
                    return None;
                }
                names.push(value.into_owned());
            }
        }
    }
    Some(names)
}

fn unknown() -> Vec<ResolvedType> {
    vec![ResolvedType::from_type_string(PhpType::mixed())]
}

/// Join all possible named reads without losing an unresolved alternative.
pub(super) fn resolve_candidates(
    names: &[String],
    mut resolve: impl FnMut(&str) -> Vec<ResolvedType>,
) -> Vec<ResolvedType> {
    let mut result = Vec::new();
    for name in names {
        let values = resolve(name);
        if values.is_empty() {
            return unknown();
        }
        ResolvedType::extend_unique(&mut result, values);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_bounded_and_unknown_alternatives_are_preserved() {
        let names: Vec<_> = (0..MAX_NAMES)
            .map(|i| {
                ResolvedType::from_type_string(PhpType::literal_string_value(format!("name{i}")))
            })
            .collect();
        assert_eq!(known_names(&names).unwrap().len(), MAX_NAMES);
        let mut repeated = names.clone();
        repeated.extend(names.clone());
        assert_eq!(known_names(&repeated).unwrap().len(), MAX_NAMES);
        let mut too_many = names.clone();
        too_many.push(ResolvedType::from_type_string(
            PhpType::literal_string_value("extra"),
        ));
        assert!(known_names(&too_many).is_none());
        let mut uncertain = names;
        uncertain.push(ResolvedType::from_type_string(PhpType::string()));
        assert!(known_names(&uncertain).is_none());
        let values = resolve_candidates(&["known".into(), "unknown".into()], |name| {
            if name == "known" {
                vec![ResolvedType::from_type_string(PhpType::int())]
            } else {
                Vec::new()
            }
        });
        assert!(values[0].type_string.is_mixed());
    }
}

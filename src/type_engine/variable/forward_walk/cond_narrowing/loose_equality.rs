use super::*;
use crate::parser::source_comparison_semantics;
use crate::php_type::ComparisonSemantics;

/// Apply what a loose comparison against a literal (`$x == 'one'`,
/// `$x != 3.5`, `$a == []`) proves about the subject, in whichever
/// direction the branch establishes.
///
/// `==` compares across types, so it pins the subject to the literal only
/// where, for the alternative in hand, it means the same as `===`: an
/// integer against an integer, a float against a float, a string against a
/// string that is not numeric (`'01' == '1'` holds, `'a' == 'b'` does not),
/// and an array against `[]`.  Every other alternative stays as it is,
/// since some of its values may compare equal.  A literal alternative is
/// kept or dropped by comparing it directly.
///
/// Boolean and null checks also participate in the dedicated condition passes.
pub(super) fn apply_loose_literal_narrowing(
    condition: &Expression<'_>,
    scope: &mut ScopeState,
    ctx: &ForwardWalkCtx<'_>,
    truthy: bool,
) {
    let (inner, negated) = condition_facts::unwrap_condition_negation(condition);
    let Expression::Binary(bin) = inner else {
        return;
    };
    let equal = match bin.operator {
        BinaryOperator::Equal(_) => !negated,
        BinaryOperator::NotEqual(_) | BinaryOperator::AngledNotEqual(_) => negated,
        _ => return,
    };
    let (subject, literal) = match (loose_comparand_type(bin.rhs), loose_comparand_type(bin.lhs)) {
        (Some(ty), _) => (bin.lhs, ty),
        (_, Some(ty)) => (bin.rhs, ty),
        _ => return,
    };
    let Some(var_name) = expr_to_subject(subject) else {
        return;
    };
    let equal = equal == truthy;
    let semantics = source_comparison_semantics();
    refine_subject(&var_name, scope, ctx, |ty| {
        if equal {
            loosely_equal_part(ty, std::slice::from_ref(&literal), semantics)
        } else {
            loosely_unequal_part(ty, std::slice::from_ref(&literal), semantics)
        }
    });
}

/// Apply a non-strict `in_array($x, [...])` check whose haystack holds only
/// literals, by the same rules as `==` against each of them.
///
/// A needle that was found equals one of the elements, whichever they
/// are, while one that was not only differs from the values the haystack
/// is known to hold (see [`values_held_by`]).  The strict form is
/// [`apply_in_array_narrowing`]'s.
pub(crate) fn apply_loose_in_array_narrowing<'b>(
    condition: &'b Expression<'b>,
    scope: &mut ScopeState,
    ctx: &ForwardWalkCtx<'_>,
    inverted: bool,
) {
    let (inner, negated) = condition_facts::unwrap_condition_negation(condition);
    let Some((needle, haystack)) = loose_in_array_target(inner) else {
        return;
    };
    let Some(var_name) = expr_to_subject(needle) else {
        return;
    };
    let Some(haystack) = resolve_in_array_haystack_type_fw(haystack, scope, ctx) else {
        return;
    };
    let found = !(inverted ^ negated);
    let literals: Vec<PhpType> = if found {
        let Some(element) = haystack.iterable_element_type() else {
            return;
        };
        element.union_members().into_iter().cloned().collect()
    } else {
        values_held_by(&haystack)
    };
    if literals.is_empty() || !literals.iter().all(is_loose_comparand) {
        return;
    }
    let semantics = source_comparison_semantics();
    refine_subject(&var_name, scope, ctx, |ty| {
        if found {
            loosely_equal_part(ty, &literals, semantics)
        } else {
            loosely_unequal_part(ty, &literals, semantics)
        }
    });
}

/// The needle and haystack of an `in_array()` call that compares loosely:
/// two arguments, or a third that is written `false`.
fn loose_in_array_target<'b>(
    expr: &'b Expression<'b>,
) -> Option<(&'b Expression<'b>, &'b Expression<'b>)> {
    let Expression::Call(Call::Function(call)) = unwrap_parens(expr) else {
        return None;
    };
    let Expression::Identifier(ident) = call.function else {
        return None;
    };
    if !crate::util::strip_fqn_prefix(bytes_to_str(ident.value())).eq_ignore_ascii_case("in_array")
    {
        return None;
    }
    let args: Vec<_> = call.argument_list.arguments.iter().collect();
    match args.len() {
        2 => {}
        3 if is_false_expr(unwrap_parens(condition_facts::argument_value(args[2]))) => {}
        _ => return None,
    }
    Some((
        condition_facts::argument_value(args[0]),
        condition_facts::argument_value(args[1]),
    ))
}

/// Replace the scope's type for `var_name` with what `refine` leaves of
/// it, unless that is nothing or no change.
pub(super) fn refine_subject(
    var_name: &str,
    scope: &mut ScopeState,
    ctx: &ForwardWalkCtx<'_>,
    refine: impl Fn(&PhpType) -> Option<PhpType>,
) {
    seed_synthetic_key_if_needed(var_name, scope, ctx);
    let types = scope.get(var_name);
    if types.is_empty() {
        return;
    }
    let mut changed = false;
    let mut narrowed = Vec::with_capacity(types.len());
    for rt in types {
        match refine(&rt.type_string) {
            Some(ty) if ty == rt.type_string || ty.equivalent(&rt.type_string) => {
                narrowed.push(rt.clone())
            }
            Some(ty) => {
                changed = true;
                narrowed.push(ResolvedType::from_type_string(ty));
            }
            None => changed = true,
        }
    }
    // Nothing left means the branch cannot run with what the subject holds,
    // which is the reachability question rather than this one.
    if changed && !narrowed.is_empty() {
        scope.set(var_name, narrowed);
        write_offset_key_into_shapes(var_name, scope);
    }
}

/// Written scalar values and the empty array can be compared directly.
fn loose_comparand_type(expr: &Expression<'_>) -> Option<PhpType> {
    literal_comparand_type(expr).filter(is_loose_comparand)
}

pub(super) fn is_loose_comparand(ty: &PhpType) -> bool {
    ty.as_literal().is_some() || is_empty_array(ty) || ty.is_null() || ty.is_true() || ty.is_false()
}

fn is_empty_array(ty: &PhpType) -> bool {
    matches!(ty.kind(), TypeKind::ArrayShape(entries) if entries.is_empty())
}

/// The part of `ty` that can loosely equal one of `literals`, or `None`
/// when no part of it can.
pub(super) fn loosely_equal_part(
    ty: &PhpType,
    literals: &[PhpType],
    semantics: ComparisonSemantics,
) -> Option<PhpType> {
    let mut kept: Vec<PhpType> = Vec::new();
    for member in expand_nullable(ty) {
        for literal in literals {
            if let Some(part) = loosely_equal_member(&member, literal, semantics) {
                kept.push(part);
            }
        }
    }
    match kept.len() {
        0 => None,
        _ => Some(PhpType::join_runtime_value_types(kept)),
    }
}

/// The part of one non-union alternative that can loosely equal `literal`.
fn loosely_equal_member(
    member: &PhpType,
    literal: &PhpType,
    semantics: ComparisonSemantics,
) -> Option<PhpType> {
    if let Some(equal) = semantics.values_equal(member, literal) {
        return equal.then(|| member.clone());
    }
    if literal.is_true() {
        return member.truthy_type();
    }
    if literal.is_false() {
        return member.falsy_type();
    }
    if literal.is_null() {
        return Some(member.clone());
    }
    if is_empty_array(literal) {
        if !member.is_array_like() {
            return Some(member.clone());
        }
        return (!member.is_provably_non_empty()).then(|| literal.clone());
    }
    let literal_value = literal.as_literal()?;
    if member.is_mixed() {
        return mixed_loosely_equal_part(literal, literal_value, semantics);
    }
    if member.as_literal().is_some() {
        // The shared comparison above could not prove either outcome.
        return Some(member.clone());
    }
    if !loose_means_identity(member, literal_value, semantics) {
        // On PHP 8 a string compares with a number, or with another string, as a
        // number only when it is numeric itself (`'abc' == 1` is false), so a
        // string equal to a numeric literal is a numeric string, though not
        // necessarily that spelling of it (`' 1' == '1'`).
        let numeric_literal = match literal_value {
            LiteralValue::String(_) => semantics.is_numeric_string(literal_value),
            LiteralValue::Int(_) | LiteralValue::Float(_) => true,
        };
        if semantics == ComparisonSemantics::Php8
            && numeric_literal
            && member.is_string_subtype()
            && member.as_literal().is_none()
        {
            return Some(PhpType::parse("numeric-string"));
        }
        return Some(member.clone());
    }
    literal.is_subtype_of(member).then(|| literal.clone())
}

/// The values of every type that the target's `==` holds equal to `literal`, for
/// a subject that could be anything.
///
/// Before PHP 8, strings contribute their numeric prefix when compared
/// to a number. Among strings, both operands must be fully numeric to
/// compare numerically. A bool equals the
/// literal when their truthiness agrees, and `null` equals `''` and `0`.
/// Arrays equal no scalar.  An object can: a `Stringable` one compares by
/// its string, and a number-like one (`GMP`, `BcMath\Number`) by its value.
fn mixed_loosely_equal_part(
    literal: &PhpType,
    value: &LiteralValue,
    semantics: ComparisonSemantics,
) -> Option<PhpType> {
    let number = semantics.number_for_comparison(value);
    if number.is_none()
        && (semantics.is_numeric_string(value) || !matches!(value, LiteralValue::String(_)))
    {
        return Some(PhpType::mixed());
    }
    let mut parts: Vec<PhpType> = Vec::new();
    match number {
        Some(number) => {
            if number.abs() >= 9_007_199_254_740_992.0 {
                // At this magnitude several integers round to the same
                // float. A single integer literal would exclude matches.
                parts.push(PhpType::int());
            } else if number.fract() == 0.0 {
                parts.push(PhpType::literal_int((number as i64).to_string()));
            }
            parts.push(PhpType::literal_float(format!("{number:?}")));
            if semantics == ComparisonSemantics::Php8 {
                parts.push(PhpType::parse("numeric-string"));
            } else if matches!(value, LiteralValue::String(_))
                && !semantics.is_numeric_string(value)
            {
                parts.push(literal.clone());
            } else {
                // Legacy numeric coercion accepts prefixes like '42foo';
                // PHP 5 also accepts hexadecimal strings.
                parts.push(PhpType::string());
            }
            if semantics != ComparisonSemantics::Php8 {
                parts.push(PhpType::parse("resource"));
            }
        }
        None => {
            // A float's string form is numeric except for `INF`, `-INF` and
            // `NAN`, which a non-numeric string can spell.
            let content = value.string_content().unwrap_or_default();
            if matches!(content.as_ref(), "INF" | "-INF" | "NAN") {
                parts.push(PhpType::float());
            }
            parts.push(literal.clone());
        }
    }
    parts.push(if literal.truthiness() == Some(true) {
        PhpType::true_()
    } else {
        PhpType::false_()
    });
    if let Some(null) = loosely_equal_member(&PhpType::null(), literal, semantics) {
        parts.push(null);
    }
    parts.push(
        if number.is_some() || semantics != ComparisonSemantics::Php8 {
            PhpType::object()
        } else {
            PhpType::named(atom("Stringable"))
        },
    );
    Some(PhpType::join_runtime_value_types(parts))
}

/// The part of `ty` that cannot loosely equal any of `literals`, or `None`
/// when nothing is left.
pub(super) fn loosely_unequal_part(
    ty: &PhpType,
    literals: &[PhpType],
    semantics: ComparisonSemantics,
) -> Option<PhpType> {
    let mut kept: Vec<PhpType> = Vec::new();
    for member in expand_nullable(ty) {
        let mut part = Some(member);
        for literal in literals {
            part = part.and_then(|m| loosely_unequal_member(&m, literal, semantics));
        }
        kept.extend(part);
    }
    match kept.len() {
        0 => None,
        1 => kept.pop(),
        _ => Some(PhpType::union(kept)),
    }
}

/// The part of one non-union alternative that cannot loosely equal
/// `literal`.  Ruling out one value of a type that holds others leaves
/// the type, except that an array unequal to `[]` has entries.
fn loosely_unequal_member(
    member: &PhpType,
    literal: &PhpType,
    semantics: ComparisonSemantics,
) -> Option<PhpType> {
    if let Some(equal) = semantics.values_equal(member, literal) {
        return (!equal).then(|| member.clone());
    }
    if literal.is_true() {
        return member.falsy_type();
    }
    if literal.is_false() {
        return member.truthy_type();
    }
    if literal.is_null() {
        return Some(member.clone());
    }
    if is_empty_array(literal) {
        if !member.is_array_like() {
            return Some(member.clone());
        }
        if is_empty_array(member) {
            return None;
        }
        return Some(member.non_empty_array_form());
    }
    Some(member.clone())
}

/// Whether `==` against `literal` holds for a value of `member` exactly
/// when `===` does.
fn loose_means_identity(
    member: &PhpType,
    literal: &LiteralValue,
    semantics: ComparisonSemantics,
) -> bool {
    match literal {
        LiteralValue::Int(_) => member.is_int_subtype(),
        LiteralValue::Float(_) => member.is_float(),
        LiteralValue::String(_) => {
            !semantics.is_numeric_string(literal) && member.is_string_subtype()
        }
    }
}

/// The alternatives of `ty`, with a nullable wrapper split into its inner
/// type and `null`.
fn expand_nullable(ty: &PhpType) -> Vec<PhpType> {
    let mut out = Vec::new();
    for member in ty.union_members() {
        match member.kind() {
            TypeKind::Nullable(inner) => {
                out.extend(inner.union_members().into_iter().cloned());
                out.push(PhpType::null());
            }
            _ => out.push(member.clone()),
        }
    }
    out
}

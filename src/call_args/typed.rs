//! Typed argument routing shared by body inference, templates and diagnostics.

use crate::php_type::{PhpType, ShapeEntry};
use crate::types::ParameterInfo;

pub(crate) struct TypedArgument<'a> {
    pub name: Option<&'a str>,
    pub ty: PhpType,
    pub spread: bool,
}

pub(crate) struct BoundArgument {
    pub parameter: usize,
    pub source: usize,
    pub ty: PhpType,
    pub key: Option<String>,
    /// The pack's length is unknown, so this value may fill this slot.
    pub uncertain: bool,
}

/// Route textual call arguments using the caller's expression resolver.
pub(crate) fn bind_typed_text_arguments(
    params: &[ParameterInfo],
    args: &[&str],
    resolve: impl Fn(&str) -> Option<PhpType>,
) -> Vec<BoundArgument> {
    bind_typed_arguments(
        params,
        args.iter()
            .filter(|text| !text.trim().is_empty())
            .map(|text| {
                let (name, text) = super::split_named_text_arg(text);
                let unpacked = super::unpacked_text_value(text);
                TypedArgument {
                    name,
                    ty: resolve(unpacked.unwrap_or(text).trim()).unwrap_or_else(PhpType::mixed),
                    spread: unpacked.is_some(),
                }
            }),
    )
}

/// Route expanded values to their declared parameters. Unknown-length packs
/// contribute their element type to every remaining possible position.
pub(crate) fn bind_typed_arguments<'a>(
    params: &[ParameterInfo],
    args: impl IntoIterator<Item = TypedArgument<'a>>,
) -> Vec<BoundArgument> {
    let mut result = Vec::new();
    let variadic = params.iter().position(|p| p.is_variadic);
    let mut next = 0;
    let mut uncertain = false;
    for (source, arg) in args.into_iter().enumerate() {
        let mut route = |name: Option<&str>, ty: &PhpType, maybe: bool| {
            if let Some(name) = name {
                if let Some(parameter) = params
                    .iter()
                    .position(|p| super::param_name_bare(p) == name)
                    .or(variadic)
                {
                    result.push(BoundArgument {
                        parameter,
                        source,
                        ty: ty.clone(),
                        key: params[parameter].is_variadic.then(|| name.to_string()),
                        uncertain: maybe,
                    });
                }
                return;
            }
            uncertain |= maybe;
            if uncertain {
                for parameter in next.min(variadic.unwrap_or(params.len()))..params.len() {
                    result.push(BoundArgument {
                        parameter,
                        source,
                        ty: ty.clone(),
                        key: None,
                        uncertain: true,
                    });
                }
            } else if let Some(parameter) = (next < params.len()).then_some(next).or(variadic) {
                result.push(BoundArgument {
                    parameter,
                    source,
                    ty: ty.clone(),
                    key: None,
                    uncertain: false,
                });
            }
            if !maybe {
                next += 1;
            }
        };
        if !arg.spread {
            route(arg.name, &arg.ty, false);
        } else if let Some(shape) = arg.ty.shape_parts() {
            for entry in shape.entries {
                let name = entry
                    .key
                    .as_deref()
                    .filter(|key| crate::php_type::canonical_int_key(key).is_none());
                route(name, &entry.value_type, entry.optional);
            }
            if let Some((_, value)) = shape.tail {
                route(None, value, true);
            }
        } else {
            route(
                None,
                &arg.ty
                    .iterable_element_type()
                    .unwrap_or_else(PhpType::mixed),
                true,
            );
        }
    }
    result
}

/// Body-local parameter types, including the array a variadic collects.
pub(crate) fn parameter_types(params: &[ParameterInfo], args: &[BoundArgument]) -> Vec<PhpType> {
    let mut types = Vec::with_capacity(params.len());
    for (index, param) in params.iter().enumerate() {
        let bound = args.iter().filter(|arg| arg.parameter == index);
        let ty = if param.is_variadic {
            if bound.clone().any(|arg| arg.uncertain) {
                PhpType::generic_array(
                    PhpType::parse("array-key"),
                    PhpType::union(bound.clone().map(|arg| arg.ty.clone()).collect()),
                )
            } else {
                let mut positional = 0;
                PhpType::array_shape(
                    bound
                        .clone()
                        .map(|arg| {
                            let key = arg.key.clone().or_else(|| {
                                let key = positional.to_string();
                                positional += 1;
                                Some(key)
                            });
                            ShapeEntry {
                                key,
                                value_type: arg.ty.clone(),
                                optional: false,
                            }
                        })
                        .collect(),
                )
            }
        } else if bound.clone().next().is_none()
            || (param.default_value.is_some() && bound.clone().any(|arg| arg.uncertain))
        {
            PhpType::mixed()
        } else {
            PhpType::union(bound.clone().map(|arg| arg.ty.clone()).collect())
        };
        types.push(ty);
    }
    if types.iter().any(PhpType::is_informative) {
        types
    } else {
        Vec::new()
    }
}

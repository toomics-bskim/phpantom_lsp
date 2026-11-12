//! Contracts the bundled stubs no longer describe for older PHP targets.

use crate::php_type::{PhpType, TypeKind};
use crate::types::{FunctionInfo, PhpVersion};

pub(super) fn apply(func: &mut FunctionInfo, version: PhpVersion) {
    if func.namespace.as_deref().is_some_and(|ns| !ns.is_empty()) {
        return;
    }
    match func.name.as_str() {
        // PHP 7.2 starts warning on non-countable values; PHP 8 throws.
        "count" | "sizeof" if version < PhpVersion::new(7, 2) => {
            parameter(func, "$value", PhpType::mixed());
        }
        "getenv" if version < PhpVersion::new(7, 1) => {
            returns(func, "string|false");
        }
        "mb_convert_encoding" if version < PhpVersion::new(7, 2) => {
            parameter(func, "$string", PhpType::string());
            returns(func, "string|false");
        }
        // Explicit null was a setter argument, unlike omitting the argument.
        // A null-based conditional cannot distinguish those calls before 8.0.
        "mb_internal_encoding" if version.major < 8 => {
            returns(func, "string|bool");
        }
        "strpos" | "stripos" | "strrpos" | "strripos" | "strstr" | "stristr"
            if version.major < 8 =>
        {
            parameter(
                func,
                "$needle",
                PhpType::parse("string|int|float|bool|null"),
            );
        }
        _ => {}
    }
    // GD and sockets lack the versioned resource annotations that curl,
    // OpenSSL, fileinfo and PostgreSQL already carry. Only their own APIs
    // are corrected; a user-defined class with the same name stays a class.
    if version.major < 8 {
        let class = if func.name.starts_with("image") {
            "GdImage"
        } else if func.name.starts_with("socket_") {
            "Socket"
        } else {
            return;
        };
        for value in [&mut func.return_type, &mut func.native_return_type]
            .into_iter()
            .flatten()
        {
            *value = resource_type(value, class);
        }
        for param in func.parameters.make_mut() {
            for value in [&mut param.type_hint, &mut param.native_type_hint]
                .into_iter()
                .flatten()
            {
                *value = resource_type(value, class);
            }
        }
    }
}

fn resource_type(ty: &PhpType, class: &str) -> PhpType {
    match ty.kind() {
        TypeKind::Named(name) | TypeKind::ClassNamed(name)
            if name.trim_start_matches('\\').eq_ignore_ascii_case(class) =>
        {
            PhpType::parse("resource")
        }
        _ => ty.map_children(&|child| resource_type(child, class)),
    }
}

fn parameter(func: &mut FunctionInfo, name: &str, ty: PhpType) {
    if let Some(param) = func
        .parameters
        .make_mut()
        .iter_mut()
        .find(|p| p.name == name)
    {
        param.type_hint = Some(ty.clone());
        param.native_type_hint = Some(ty);
    }
}

fn returns(func: &mut FunctionInfo, ty: &str) {
    func.return_type = Some(PhpType::parse(ty));
    func.conditional_return = None;
}

//! Syntax checks for types written by code actions and completion.

use super::PhpVersion;
use crate::php_type::{PhpType, TypeKind};

impl PhpVersion {
    /// Whether a type can be written in a generated parameter declaration.
    /// A PHPDoc pseudo-type must never become a class constraint.
    pub fn supports_parameter_hint(self, ty: &PhpType) -> bool {
        self.native_hint(ty, false, false)
    }

    /// Whether a type can be written in a generated return declaration.
    pub fn supports_return_hint(self, ty: &PhpType) -> bool {
        self.supports_return_types() && self.native_hint(ty, true, false)
    }

    fn native_hint(self, ty: &PhpType, returns: bool, in_union: bool) -> bool {
        match ty.kind() {
            // Native PHP 5 hints can be class names that look like primitives.
            TypeKind::ClassNamed(_) => true,
            TypeKind::Named(name) => match name.as_str() {
                "resource" | "open-resource" | "closed-resource" => false,
                "static" => returns && self >= Self::new(8, 0),
                "void" => returns && !in_union && self.supports_void_return_type(),
                "never" => returns && !in_union && self >= Self::new(8, 1),
                "mixed" => !in_union && self >= Self::new(8, 0),
                "false" | "null" => self >= Self::new(8, if in_union { 0 } else { 2 }),
                "true" => self >= Self::new(8, 2),
                "array" | "callable" | "bool" | "int" | "float" | "string" | "iterable"
                | "object" => self.supports_builtin_type(name),
                _ => ty.to_native_hint_typed().as_ref() == Some(ty),
            },
            TypeKind::StaticType(_) | TypeKind::ThisType(_) => {
                returns && self >= Self::new(8, 0)
            }
            TypeKind::Nullable(inner) => {
                self >= Self::new(7, 1)
                    && !matches!(inner.kind(), TypeKind::Named(name) if matches!(name.as_str(), "mixed" | "void" | "never" | "null"))
                    && self.native_hint(inner, returns, false)
            }
            TypeKind::Union(members) => self >= Self::new(8, 0)
                && members.iter().all(|member| {
                    (!matches!(member.kind(), TypeKind::Intersection(_)) || self >= Self::new(8, 2))
                        && self.native_hint(member, returns, true)
                }),
            TypeKind::Intersection(members) => self >= Self::new(8, 1)
                && members.iter().all(|member| matches!(member.kind(), TypeKind::ClassNamed(_))
                    || matches!(member.kind(), TypeKind::Named(name) if crate::php_type::is_class_like_name(name))),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_type_syntax_respects_boundaries_and_pseudo_types() {
        for (text, parameter_since, return_since) in [
            ("DateTime", Some((5, 5)), Some((7, 0))),
            ("array", Some((5, 5)), Some((7, 0))),
            ("int", Some((7, 0)), Some((7, 0))),
            ("?DateTime", Some((7, 1)), Some((7, 1))),
            ("iterable", Some((7, 1)), Some((7, 1))),
            ("object", Some((7, 2)), Some((7, 2))),
            ("int|string", Some((8, 0)), Some((8, 0))),
            ("DateTime&Countable", Some((8, 1)), Some((8, 1))),
            ("(DateTime&Countable)|Exception", Some((8, 2)), Some((8, 2))),
            ("false", Some((8, 2)), Some((8, 2))),
            ("void", None, Some((7, 1))),
            ("never", None, Some((8, 1))),
            ("static", None, Some((8, 0))),
            ("resource", None, None),
            ("open-resource", None, None),
            ("scalar", None, None),
            ("positive-int", None, None),
            ("array{value:int}", None, None),
        ] {
            for version in PhpVersion::TARGETS {
                let ty = PhpType::parse(text);
                assert_eq!(
                    version.supports_parameter_hint(&ty),
                    parameter_since.is_some_and(|(a, b)| version >= PhpVersion::new(a, b)),
                    "{version}: parameter {text}"
                );
                assert_eq!(
                    version.supports_return_hint(&ty),
                    return_since.is_some_and(|(a, b)| version >= PhpVersion::new(a, b)),
                    "{version}: return {text}"
                );
            }
        }
        let class = PhpType::class_named(crate::atom::atom("resource"));
        assert!(PhpVersion::new(5, 5).supports_parameter_hint(&class));
        assert!(
            PhpVersion::new(5, 5).supports_parameter_hint(&PhpType::parse("\\Domain\\resource"))
        );
    }
}

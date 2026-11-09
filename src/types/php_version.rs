//! The PHP version the project targets, used for stub variants and
//! language capabilities. Promotion and property-hook capability checks
//! are adapted from PHPStan; see THIRD_PARTY_NOTICES.md.

use super::*;

/// A PHP major.minor version used for stub filtering and language capabilities.
///
/// phpstorm-stubs annotate functions, methods, and parameters with
/// `#[PhpStormStubsElementAvailable(from: 'X.Y', to: 'X.Y')]` attributes
/// to indicate which PHP versions they apply to.  PHPantom uses this
/// struct to decide which variant of a stub element to present.
///
/// The version is selected from explicit configuration or root Composer
/// requirements during initialization. When none is usable, [`PhpVersion::default`]
/// returns PHP 8.5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PhpVersion {
    /// Major version number (e.g. `8` in PHP 8.4).
    pub major: u8,
    /// Minor version number (e.g. `4` in PHP 8.4).
    pub minor: u8,
}

impl PhpVersion {
    /// Create a new `PhpVersion`.
    pub const fn new(major: u8, minor: u8) -> Self {
        Self { major, minor }
    }

    /// Whether functions may declare a native return type.
    pub fn supports_return_types(self) -> bool {
        self >= Self::new(7, 0)
    }

    /// Whether a native return declaration may use `void`.
    pub fn supports_void_return_type(self) -> bool {
        self >= Self::new(7, 1)
    }

    /// Whether constructor parameters may declare properties.
    pub fn supports_promoted_properties(self) -> bool {
        self >= Self::new(8, 0)
    }

    /// Whether properties may declare `get` and `set` hooks.
    pub fn supports_property_hooks(self) -> bool {
        self >= Self::new(8, 4)
    }

    /// Whether a lowercase native type name has its built-in meaning at this version.
    /// Class names and PHPDoc-only types are not built-in native types.
    /// The introduction versions are adapted from nikic/PHP-Parser;
    /// see THIRD_PARTY_NOTICES.md.
    pub fn supports_builtin_type(self, name: &str) -> bool {
        let minimum = match name {
            "array" => Self::new(5, 1),
            "callable" => Self::new(5, 4),
            "bool" | "int" | "float" | "string" => Self::new(7, 0),
            "iterable" | "void" => Self::new(7, 1),
            "object" => Self::new(7, 2),
            "null" | "false" | "mixed" => Self::new(8, 0),
            "never" => Self::new(8, 1),
            "true" => Self::new(8, 2),
            _ => return false,
        };
        self >= minimum
    }

    /// The language versions for which automatic target selection is defined.
    pub const TARGETS: [Self; 13] = [
        Self::new(5, 5),
        Self::new(5, 6),
        Self::new(7, 0),
        Self::new(7, 1),
        Self::new(7, 2),
        Self::new(7, 3),
        Self::new(7, 4),
        Self::new(8, 0),
        Self::new(8, 1),
        Self::new(8, 2),
        Self::new(8, 3),
        Self::new(8, 4),
        Self::new(8, 5),
    ];

    /// Select the lowest supported language family admitted by a Composer constraint.
    /// Patch bounds participate in constraint intersection, then the selected family
    /// is reduced to major.minor. This does not resolve installed runtime releases.
    pub fn from_composer_constraint(constraint: &str) -> Option<Self> {
        use semver_php::{Intervals, MultiConstraint, Operator, SingleConstraint, VersionParser};

        if constraint.trim().is_empty() {
            return None;
        }
        // Common requirements already name their minimum family. Avoid compiling
        // Composer's constraint regexes at startup for these unambiguous forms.
        let text = constraint.trim();
        let exact = text
            .strip_prefix('^')
            .or_else(|| text.strip_prefix('~'))
            .or_else(|| text.strip_prefix(">="))
            .unwrap_or(text);
        if exact == exact.trim()
            && let Some(version) = Self::from_exact(exact)
            && Self::TARGETS.contains(&version)
        {
            return Some(version);
        }
        let parsed = VersionParser::parse_constraints(constraint).ok()?;
        let mut intervals = Intervals::new();
        Self::TARGETS.into_iter().find(|version| {
            let family = MultiConstraint::new(
                vec![
                    Box::new(SingleConstraint::new(
                        Operator::Ge,
                        format!("{}.{}.0.0-dev", version.major, version.minor),
                    )),
                    Box::new(SingleConstraint::new(
                        Operator::Lt,
                        format!("{}.{}.0.0-dev", version.major, version.minor + 1),
                    )),
                ],
                true,
            );
            intervals.have_intersections(parsed.as_ref(), &family)
        })
    }

    /// Read an exact numeric version, without treating operators as an override.
    /// A patch component is accepted but does not change the language family.
    pub fn from_exact(value: &str) -> Option<Self> {
        let mut parts = value.trim().split('.');
        let number = |part: &str| {
            (!part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
                .then(|| part.parse::<u8>().ok())
                .flatten()
        };
        let major = number(parts.next()?)?;
        let minor = number(parts.next()?)?;
        if let Some(patch) = parts.next()
            && (patch.is_empty() || !patch.bytes().all(|byte| byte.is_ascii_digit()))
        {
            return None;
        }
        if parts.next().is_some() {
            return None;
        }
        Some(Self::new(major, minor))
    }

    /// Read the version at the start of stub metadata, allowing a description after it.
    /// This stays allocation-free when scanning every declaration during startup.
    pub fn from_metadata(value: &str) -> Option<Self> {
        Self::from_exact(value.split_whitespace().next()?)
    }

    /// Returns `true` if the given `from`..`to` version range includes
    /// this PHP version.
    ///
    /// - `from` is inclusive: the element is available starting at that version.
    /// - `to` is inclusive: the element is available up to and including that version.
    /// - When `from` is `None`, there is no lower bound.
    /// - When `to` is `None`, there is no upper bound.
    pub fn matches_range(&self, from: Option<PhpVersion>, to: Option<PhpVersion>) -> bool {
        if let Some(lower) = from
            && (self.major, self.minor) < (lower.major, lower.minor)
        {
            return false;
        }
        if let Some(upper) = to
            && (self.major, self.minor) > (upper.major, upper.minor)
        {
            return false;
        }
        true
    }
}

impl Default for PhpVersion {
    /// Default PHP version when none is detected: 8.5.
    fn default() -> Self {
        Self { major: 8, minor: 5 }
    }
}

impl fmt::Display for PhpVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

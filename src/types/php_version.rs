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
/// The version is detected from `composer.json` (`require.php`) during
/// server initialization. When no version is found, [`PhpVersion::default`]
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

    /// Parse a PHP version from a Composer `require.php` constraint string.
    ///
    /// Extracts the first `major.minor` pair found in the constraint.
    /// Supports common formats:
    ///   - `"^8.4"` → 8.4
    ///   - `">=8.3"` → 8.3
    ///   - `"~8.2"` → 8.2
    ///   - `"8.1.*"` → 8.1
    ///   - `">=8.0 <8.4"` → 8.0 (first match wins)
    ///   - `"8.3.1"` → 8.3
    ///   - `"^8"` → 8.0
    ///
    /// Returns `None` if no version can be extracted.
    pub fn from_composer_constraint(constraint: &str) -> Option<Self> {
        // Walk through the constraint looking for digit sequences that
        // form a major.minor version.  Skip common prefix operators.
        let s = constraint.trim();

        // Try each whitespace/pipe-separated segment, return the first match.
        for segment in s.split(['|', ' ']) {
            let segment = segment.trim();
            if segment.is_empty() {
                continue;
            }

            // Strip leading operator characters: ^, ~, >=, <=, >, <, =, !
            let digits_start = segment
                .find(|c: char| c.is_ascii_digit())
                .unwrap_or(segment.len());
            let version_part = &segment[digits_start..];

            if version_part.is_empty() {
                continue;
            }

            let mut parts = version_part.split('.');
            if let Some(major_str) = parts.next()
                && let Ok(major) = major_str.parse::<u8>()
            {
                let minor = parts
                    .next()
                    .and_then(|s| s.trim_end_matches('*').parse::<u8>().ok())
                    .unwrap_or(0);
                return Some(Self { major, minor });
            }
        }

        None
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

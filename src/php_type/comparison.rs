//! Versioned scalar coercions used by loose equality and switch selection.

use super::{LiteralValue, PhpType, TypeKind};

/// Scalar comparison rules at PHP's numeric-string compatibility boundaries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComparisonSemantics {
    /// Hexadecimal strings and leading numeric prefixes are numeric.
    Php5,
    /// Hexadecimal strings are no longer numeric.
    Php7,
    /// Numbers no longer coerce non-numeric strings to numbers.
    Php8,
}

impl ComparisonSemantics {
    /// Select the comparison rules for a supported PHP major version.
    pub fn from_major(major: u8) -> Self {
        match major {
            0..=5 => Self::Php5,
            6..=7 => Self::Php7,
            _ => Self::Php8,
        }
    }

    /// PHP 5 accepts unsigned hexadecimal strings. PHP 8 additionally
    /// accepts trailing whitespace in a fully numeric string.
    pub fn is_numeric_string(self, value: &LiteralValue) -> bool {
        value.string_content().is_some_and(|text| {
            numeric_prefix(&text, self).is_some_and(|(prefix, _)| {
                let text = text.trim_start_matches(php_whitespace);
                let rest = &text[prefix.len()..];
                rest.is_empty() || (self == Self::Php8 && rest.bytes().all(php_space_byte))
            })
        })
    }

    /// One known scalar/empty-array value compared to another. Unknown
    /// types return no answer, rather than proving that a branch is dead.
    pub fn values_equal(self, left: &PhpType, right: &PhpType) -> Option<bool> {
        let known = |ty: &PhpType| {
            ty.as_literal().is_some()
                || ty.is_null()
                || ty.is_true()
                || ty.is_false()
                || matches!(ty.kind(), TypeKind::ArrayShape(entries) if entries.is_empty())
        };
        if !known(left) || !known(right) {
            return None;
        }
        if left.is_true() || left.is_false() || right.is_true() || right.is_false() {
            return Some(left.truthiness()? == right.truthiness()?);
        }
        if left.is_null() || right.is_null() {
            let other = if left.is_null() { right } else { left };
            return Some(match other.as_literal() {
                Some(LiteralValue::String(_)) => other.as_literal()?.string_content()?.is_empty(),
                _ => !other.truthiness()?,
            });
        }
        match (left.as_literal(), right.as_literal()) {
            (Some(a), Some(b)) => self.literals_equal(a, b),
            (None, None) => Some(true),
            _ => Some(false),
        }
    }

    /// Compare literals, preserving uncertainty for unreadable or platform-dependent values.
    pub fn literals_equal(self, left: &LiteralValue, right: &LiteralValue) -> Option<bool> {
        let result = self.equal_with_int_bits(left, right, 64)?;
        // PHP 5 Windows builds use 32-bit integers even on x64. Without a
        // target architecture, a width-dependent result is not a proof.
        if self == Self::Php5 && self.equal_with_int_bits(left, right, 32)? != result {
            return None;
        }
        Some(result)
    }

    fn equal_with_int_bits(
        self,
        left: &LiteralValue,
        right: &LiteralValue,
        bits: u8,
    ) -> Option<bool> {
        if let (Some(a), Some(b)) = (left.string_content(), right.string_content()) {
            if a == b {
                return Some(true);
            }
            if !self.is_numeric_string(left) || !self.is_numeric_string(right) {
                return Some(false);
            }
            let a_number = number_from_string(&a, self, bits)?;
            let b_number = number_from_string(&b, self, bits)?;
            if a_number.overflow
                && b_number.overflow
                && a_number.as_float() == b_number.as_float()
                && (bits == 64 || a_number.as_float().abs() > 9_007_199_254_740_991.0)
            {
                return Some(false);
            }
            if a_number.overflow != b_number.overflow
                && (a_number.integer.is_some() || b_number.integer.is_some())
            {
                return Some(false);
            }
            return Some(a_number.equals(b_number));
        }
        if self == Self::Php8 {
            for (value, number) in [(left, right), (right, left)] {
                if value.string_content().is_some() && !self.is_numeric_string(value) {
                    self.number(number, bits)?;
                    return Some(false);
                }
            }
        }
        Some(self.number(left, bits)?.equals(self.number(right, bits)?))
    }

    fn number(self, value: &LiteralValue, bits: u8) -> Option<Number> {
        match value {
            LiteralValue::Int(_) => {
                let value = value.parse_i64()?;
                Some(Number::integer(value, bits))
            }
            LiteralValue::Float(_) => Number::float(value.parse_f64()?, false),
            LiteralValue::String(_) => number_from_string(&value.string_content()?, self, bits),
        }
    }

    /// Numeric value used when a string meets a number, including PHP 5/7
    /// prefixes and their zero conversion for non-numeric strings.
    pub(crate) fn number_for_comparison(self, value: &LiteralValue) -> Option<f64> {
        if matches!(value, LiteralValue::String(_))
            && self == Self::Php8
            && !self.is_numeric_string(value)
        {
            return None;
        }
        Some(self.number(value, 64)?.as_float())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versioned_numeric_string_grammar() {
        for (text, expected) in [
            ("42", [true, true, true]),
            (" 42", [true, true, true]),
            ("\u{b}42", [true, true, true]),
            ("\u{c}42", [true, true, true]),
            ("42 ", [false, false, true]),
            ("42\t", [false, false, true]),
            ("0x10", [true, false, false]),
            ("+0x10", [false, false, false]),
            ("-0x10", [false, false, false]),
            ("0Xf", [true, false, false]),
            ("1e+2", [true, true, true]),
            ("1e+", [false, false, false]),
            (".5", [true, true, true]),
            ("0b10", [false, false, false]),
            ("012", [true, true, true]),
            ("INF", [false, false, false]),
            ("\u{a0}42", [false, false, false]),
        ] {
            for (semantics, expected) in [
                ComparisonSemantics::Php5,
                ComparisonSemantics::Php7,
                ComparisonSemantics::Php8,
            ]
            .into_iter()
            .zip(expected)
            {
                assert_eq!(
                    semantics.is_numeric_string(&LiteralValue::string_value(text)),
                    expected,
                    "{semantics:?}: {text:?}"
                );
            }
        }
    }

    #[test]
    fn scalar_comparisons_follow_version_and_operand_types() {
        for (left, right, expected) in [
            ("0", "'foo'", [true, true, false]),
            ("0", "''", [true, true, false]),
            ("42", "'42foo'", [true, true, false]),
            ("100", "'1e2tail'", [true, true, false]),
            ("1", "'1e+'", [true, true, false]),
            ("0.5", "'.5foo'", [true, true, false]),
            ("16", "'0x10'", [true, false, false]),
            ("0", "'+0x10'", [true, true, false]),
            ("'16'", "'0x10'", [true, false, false]),
            ("'42'", "'42 '", [false, false, true]),
            ("42", "'42 '", [true, true, true]),
            ("'42'", "'42foo'", [false, false, false]),
            ("'foo'", "'bar'", [false, false, false]),
            ("false", "'0'", [true, true, true]),
            ("true", "'foo'", [true, true, true]),
            ("null", "''", [true, true, true]),
            ("null", "'0'", [false, false, false]),
            ("null", "0", [true, true, true]),
            ("null", "array{}", [true, true, true]),
            ("false", "array{}", [true, true, true]),
            ("0", "array{}", [false, false, false]),
            (
                "'9223372036854775808'",
                "'9223372036854775809'",
                [false, false, false],
            ),
        ] {
            let (a, b) = (PhpType::parse(left), PhpType::parse(right));
            for (semantics, expected) in [
                ComparisonSemantics::Php5,
                ComparisonSemantics::Php7,
                ComparisonSemantics::Php8,
            ]
            .into_iter()
            .zip(expected)
            {
                assert_eq!(
                    semantics.values_equal(&a, &b),
                    Some(expected),
                    "{semantics:?}: {left} == {right}"
                );
                assert_eq!(
                    semantics.values_equal(&b, &a),
                    Some(expected),
                    "{semantics:?}: {right} == {left}"
                );
            }
        }
    }

    #[test]
    fn large_integers_do_not_gain_float_equality() {
        let a = LiteralValue::int("9007199254740992");
        let b = LiteralValue::string_value("9007199254740993");
        assert_eq!(ComparisonSemantics::Php5.literals_equal(&a, &b), None);
        assert_eq!(
            ComparisonSemantics::Php7.literals_equal(&a, &b),
            Some(false)
        );
        assert_eq!(
            ComparisonSemantics::Php8.literals_equal(&a, &b),
            Some(false)
        );
        assert_eq!(a.identical_to(&b), Some(false));
        assert_eq!(
            ComparisonSemantics::Php5.values_equal(&PhpType::int(), &PhpType::literal_int("0")),
            None
        );
    }
}

#[derive(Clone, Copy)]
struct Number {
    integer: Option<i64>,
    value: f64,
    overflow: bool,
}

impl Number {
    fn integer(value: i64, bits: u8) -> Self {
        let fits = bits == 64 || i32::try_from(value).is_ok();
        Self {
            integer: fits.then_some(value),
            value: value as f64,
            overflow: !fits,
        }
    }

    fn float(value: f64, overflow: bool) -> Option<Self> {
        value.is_finite().then_some(Self {
            integer: None,
            value,
            overflow,
        })
    }

    fn as_float(self) -> f64 {
        self.value
    }

    fn equals(self, other: Self) -> bool {
        match (self.integer, other.integer) {
            (Some(a), Some(b)) => a == b,
            _ => self.value == other.value,
        }
    }
}

fn number_from_string(text: &str, semantics: ComparisonSemantics, bits: u8) -> Option<Number> {
    let Some((prefix, radix)) = numeric_prefix(text, semantics) else {
        return Some(Number::integer(0, bits));
    };
    if radix == 16 {
        if let Ok(value) = i64::from_str_radix(&prefix[2..], 16) {
            return Some(Number::integer(value, bits));
        }
        let value = prefix[2..].bytes().fold(0.0, |value, digit| {
            value * 16.0 + (digit as char).to_digit(16).unwrap_or(0) as f64
        });
        return Number::float(value, true);
    }
    let integer = !prefix.contains(['.', 'e', 'E']);
    if integer && let Ok(value) = prefix.parse::<i64>() {
        return Some(Number::integer(value, bits));
    }
    Number::float(prefix.parse().ok()?, integer)
}

fn php_space_byte(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

fn php_whitespace(ch: char) -> bool {
    ch.is_ascii() && php_space_byte(ch as u8)
}

/// Longest numeric prefix, borrowing the original string. Signs do not
/// belong to PHP 5's hexadecimal form, and an incomplete exponent stops
/// before the `e` rather than invalidating the preceding decimal digits.
fn numeric_prefix(text: &str, semantics: ComparisonSemantics) -> Option<(&str, u32)> {
    let text = text.trim_start_matches(php_whitespace);
    let bytes = text.as_bytes();
    if semantics == ComparisonSemantics::Php5
        && bytes.len() > 2
        && bytes[0] == b'0'
        && matches!(bytes[1], b'x' | b'X')
        && bytes[2].is_ascii_hexdigit()
    {
        let end = 2 + bytes[2..]
            .iter()
            .take_while(|b| b.is_ascii_hexdigit())
            .count();
        return Some((&text[..end], 16));
    }
    let mut end = usize::from(bytes.first().is_some_and(|b| matches!(b, b'+' | b'-')));
    let start = end;
    while bytes.get(end).is_some_and(u8::is_ascii_digit) {
        end += 1;
    }
    let mut digits = end - start;
    if bytes.get(end) == Some(&b'.') {
        end += 1;
        let start = end;
        while bytes.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
        }
        digits += end - start;
    }
    if digits == 0 {
        return None;
    }
    if bytes.get(end).is_some_and(|b| matches!(b, b'e' | b'E')) {
        let mut exponent = end + 1;
        if bytes
            .get(exponent)
            .is_some_and(|b| matches!(b, b'+' | b'-'))
        {
            exponent += 1;
        }
        let start = exponent;
        while bytes.get(exponent).is_some_and(u8::is_ascii_digit) {
            exponent += 1;
        }
        if exponent > start {
            end = exponent;
        }
    }
    Some((&text[..end], 10))
}

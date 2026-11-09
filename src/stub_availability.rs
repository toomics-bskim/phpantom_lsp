//! PHP release metadata shared by the embedded-index generator and stub parsing.

use mago_allocator::LocalArena;
use mago_database::file::FileId;
use mago_names::{ResolvedNames, resolver::NameResolver};
use mago_span::HasSpan;
use mago_syntax::{cst::*, walker::MutWalker};

/// Introduction and `to` are inclusive; removal is exclusive. Versions use major * 100 + minor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Availability {
    /// First supported release, inclusive.
    pub from: u16,
    /// Last supported release, inclusive.
    pub to: u16,
    /// First unsupported release, exclusive.
    pub removed: u16,
}

impl Default for Availability {
    fn default() -> Self {
        Self {
            from: 0,
            to: u16::MAX,
            removed: u16::MAX,
        }
    }
}

impl Availability {
    /// Whether this declaration exists in the selected PHP release.
    pub fn contains(self, version: u16) -> bool {
        version >= self.from && version <= self.to && version < self.removed
    }
}

/// Read a numeric PHP release, without interpreting package constraints.
pub fn version(text: &str) -> Option<u16> {
    let token = text.split_whitespace().next()?;
    let mut parts = token.split('.');
    let number = |part: &str| {
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        part.parse::<u8>().ok()
    };
    let major = number(parts.next()?)?;
    let minor = number(parts.next()?)?;
    if let Some(patch) = parts.next()
        && (patch.is_empty() || !patch.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    if parts.next().is_some() {
        return None;
    }
    Some(u16::from(major) * 100 + u16::from(minor))
}

/// PHP-specific attributes and removal tags apply to all stubs; `@since`
/// requires trusted provenance because libraries also use it for package versions.
pub fn bounds(
    docblock: Option<&str>,
    attributes: &Sequence<'_, AttributeList<'_>>,
    is_availability: impl Fn(&Identifier<'_>) -> bool,
    trust_docblocks: bool,
) -> Availability {
    let mut result = Availability::default();
    if let Some(docblock) = docblock {
        for line in docblock.lines() {
            let line = line
                .trim()
                .trim_start_matches("/**")
                .trim_start_matches('*')
                .trim();
            let mut words = line.split_whitespace();
            let tag = words.next();
            let Some(value) = words.next().and_then(version) else {
                continue;
            };
            match tag {
                Some("@since") if trust_docblocks => {
                    result.from = if result.from == 0 {
                        value
                    } else {
                        result.from.min(value)
                    }
                }
                Some("@removed") => result.removed = result.removed.min(value),
                _ => {}
            }
        }
    }
    for list in attributes.iter() {
        for attr in list
            .attributes
            .iter()
            .filter(|attr| is_availability(&attr.name))
        {
            let Some(args) = &attr.argument_list else {
                continue;
            };
            for arg in args.arguments.iter() {
                let (name, expression) = match arg {
                    PartialArgument::Named(arg) => (arg.name.value, arg.value),
                    PartialArgument::Positional(arg) => (b"from".as_slice(), arg.value),
                    _ => continue,
                };
                let Some(value) = string(expression).and_then(version) else {
                    continue;
                };
                match name {
                    b"from" => result.from = result.from.max(value),
                    b"to" => result.to = result.to.min(value),
                    _ => {}
                }
            }
        }
    }
    result
}

fn string<'a>(expression: &Expression<'a>) -> Option<&'a str> {
    if let Expression::Literal(Literal::String(literal)) = expression {
        literal
            .value
            .and_then(|value| std::str::from_utf8(value).ok())
    } else {
        None
    }
}

/// Global declaration; repeated variants are unioned by consumers.
pub struct Declaration {
    /// Class = 0, function = 1, constant = 2.
    pub kind: u8,
    /// Fully qualified declaration name.
    pub name: String,
    /// PHP releases containing this variant.
    pub availability: Availability,
}

/// Corrections for missing release tags in bundled PHP stubs only.
pub fn corrected_bounds(kind: u8, name: &str, mut bounds: Availability) -> Availability {
    // https://www.php.net/manual/en/migration70.constants.php
    if kind == 2 && name == "PHP_INT_MIN" {
        bounds.from = bounds.from.max(700);
    }
    bounds
}

/// Read global declarations with real PHP scopes, aliases, and attached docblocks.
pub fn declarations(content: &str, trust_docblocks: bool) -> Vec<Declaration> {
    let arena = LocalArena::new();
    let program = mago_syntax::parser::parse_file_content(
        &arena,
        FileId::new(b"stub.php"),
        content.as_bytes(),
    );
    let names = NameResolver::new(&arena).resolve(program);
    let mut collector = Collector {
        program,
        names: &names,
        trust_docblocks,
        declarations: Vec::new(),
    };
    collector.walk_program(program, &mut ());
    collector.declarations
}

struct Collector<'a, 'b> {
    program: &'a Program<'a>,
    names: &'b ResolvedNames<'a>,
    trust_docblocks: bool,
    declarations: Vec<Declaration>,
}

impl Collector<'_, '_> {
    fn add(
        &mut self,
        kind: u8,
        name: String,
        node: &impl HasSpan,
        attributes: &Sequence<'_, AttributeList<'_>>,
    ) {
        let docblock = mago_syntax::comments::docblock::get_docblock_before_position(
            self.program.trivia.as_slice(),
            node.span().start.offset,
        )
        .and_then(|trivia| std::str::from_utf8(trivia.value).ok());
        let availability = bounds(
            docblock,
            attributes,
            |name| {
                let raw = self
                    .names
                    .resolve(&name.span().start)
                    .unwrap_or(name.value());
                raw.rsplit(|b| *b == b'\\').next() == Some(b"PhpStormStubsElementAvailable")
            },
            self.trust_docblocks,
        );
        let availability = if self.trust_docblocks {
            corrected_bounds(kind, &name, availability)
        } else {
            availability
        };
        self.declarations.push(Declaration {
            kind,
            name,
            availability,
        });
    }

    fn name(&self, name: &LocalIdentifier<'_>) -> String {
        String::from_utf8_lossy(self.names.resolve(&name.span.start).unwrap_or(name.value))
            .into_owned()
    }
}

impl<'a> MutWalker<'a, 'a, ()> for Collector<'a, '_> {
    fn walk_class(&mut self, node: &'a Class<'a>, _: &mut ()) {
        self.add(0, self.name(&node.name), node, &node.attribute_lists);
    }
    fn walk_interface(&mut self, node: &'a Interface<'a>, _: &mut ()) {
        self.add(0, self.name(&node.name), node, &node.attribute_lists);
    }
    fn walk_trait(&mut self, node: &'a Trait<'a>, _: &mut ()) {
        self.add(0, self.name(&node.name), node, &node.attribute_lists);
    }
    fn walk_enum(&mut self, node: &'a Enum<'a>, _: &mut ()) {
        self.add(0, self.name(&node.name), node, &node.attribute_lists);
    }
    fn walk_function(&mut self, node: &'a Function<'a>, _: &mut ()) {
        self.add(1, self.name(&node.name), node, &node.attribute_lists);
    }
    fn walk_constant(&mut self, node: &'a Constant<'a>, _: &mut ()) {
        for item in node.items.iter() {
            self.add(2, self.name(&item.name), node, &node.attribute_lists);
        }
    }
    fn walk_statement_expression(&mut self, node: &'a ExpressionStatement<'a>, _: &mut ()) {
        let Expression::Call(Call::Function(call)) = node.expression else {
            return;
        };
        let Expression::Identifier(name) = call.function else {
            return;
        };
        if !name
            .value()
            .strip_prefix(b"\\")
            .unwrap_or(name.value())
            .eq_ignore_ascii_case(b"define")
        {
            return;
        }
        let Some(arg) = call.argument_list.arguments.iter().next() else {
            return;
        };
        let value = match arg {
            Argument::Named(arg) => arg.value,
            Argument::Positional(arg) => arg.value,
        };
        if let Some(name) = string(value) {
            self.add(2, name.to_owned(), node, &Sequence::empty());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_release_tags_do_not_accept_package_constraints() {
        assert_eq!(version("5.6.0 Added to PHP"), Some(506));
        for invalid in ["^5.6", ">=5.6", "v5.6", "+5.6", "5.6-beta", "5.6.0.1"] {
            assert_eq!(version(invalid), None, "{invalid}");
        }
    }

    #[test]
    fn availability_uses_scopes_aliases_and_attached_comments() {
        let source = r#"<?php
namespace First {
use JetBrains\PhpStorm\Internal\PhpStormStubsElementAvailable as Available;
/** @since 5.6 */
#[Available(to: '7.4')]
function same() {}
class Container {
    /** @since 8.0 */
    public function same() {}
}
// @removed 5.0 function untouched() {}
function untouched() { $text = '/** @since 9.0 */ function fake() {}'; }
/** @since 7.0 */
const NEW_CONSTANT = 1;
}
namespace Second {
/** @removed 7.0 */
function same() {}
/** @since 5.6 */
\define('GLOBAL_CONSTANT', 1);
}
"#;
        for text in [source.to_owned(), source.replace('\n', "\r\n")] {
            let declarations = declarations(&text, true);
            assert_eq!(declarations.len(), 6);
            let bounds = |name| {
                declarations
                    .iter()
                    .find(|d| d.name == name)
                    .unwrap()
                    .availability
            };
            assert!(!bounds("First\\same").contains(505));
            assert!(bounds("First\\same").contains(506));
            assert!(!bounds("First\\same").contains(800));
            assert!(bounds("First\\untouched").contains(505));
            assert!(bounds("Second\\same").contains(505));
            assert!(!bounds("Second\\same").contains(700));
            assert!(!bounds("First\\NEW_CONSTANT").contains(506));
            assert!(!bounds("GLOBAL_CONSTANT").contains(505));
        }
    }

    #[test]
    fn package_since_is_separate_from_php_specific_attributes() {
        let source = "<?php\n/** @since 9.0 */\nfunction package() {}\n#[\\JetBrains\\PhpStorm\\Internal\\PhpStormStubsElementAvailable(from: '5.6')]\nfunction php() {}\n";
        let declarations = declarations(source, false);
        assert!(declarations[0].availability.contains(505));
        assert!(!declarations[1].availability.contains(505));
        assert!(declarations[1].availability.contains(506));
    }
}

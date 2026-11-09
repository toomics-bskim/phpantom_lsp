use crate::T;
use crate::cst::cst::Modifier;
use crate::cst::sequence::Sequence;
use crate::error::ParseError;
use crate::parser::Parser;
use mago_allocator::prelude::*;

impl<'arena, A> Parser<'_, 'arena, A>
where
    A: Arena,
{
    pub(crate) fn parse_modifier_sequence(&mut self) -> Result<Sequence<'arena, Modifier<'arena>>, ParseError> {
        let mut modifiers = self.new_vec();
        while let Some(modifier) = self.parse_optional_modifier()? {
            modifiers.push(modifier);
        }

        Ok(Sequence::new(modifiers))
    }

    pub(crate) fn is_legacy_readonly_modifier(&mut self) -> Result<bool, ParseError> {
        Ok(self.php_version < mago_php_version::PHPVersion::PHP81
            && self.stream.lookahead(0)?.is_some_and(|token| {
                token.kind == T![Identifier] && token.value.eq_ignore_ascii_case(b"readonly")
            })
            && !matches!(self.stream.peek_kind(1)?,
                Some(T!["$variable" | "(" | "::" | "=" | ";" | "|" | "&" | "..."]) | None))
    }

    pub(crate) fn parse_optional_modifier(&mut self) -> Result<Option<Modifier<'arena>>, ParseError> {
        if self.is_legacy_readonly_modifier()? {
            return Ok(Some(Modifier::Readonly(self.expect_any_keyword()?)));
        }
        Ok(Some(match self.stream.peek_kind(0)? {
            Some(T!["public"]) => Modifier::Public(self.expect_any_keyword()?),
            Some(T!["protected"]) => Modifier::Protected(self.expect_any_keyword()?),
            Some(T!["private"]) => Modifier::Private(self.expect_any_keyword()?),
            Some(T!["static"]) => Modifier::Static(self.expect_any_keyword()?),
            Some(T!["final"]) => Modifier::Final(self.expect_any_keyword()?),
            Some(T!["abstract"]) => Modifier::Abstract(self.expect_any_keyword()?),
            Some(T!["readonly"]) => Modifier::Readonly(self.expect_any_keyword()?),
            Some(T!["private(set)"]) => Modifier::PrivateSet(self.expect_any_keyword()?),
            Some(T!["protected(set)"]) => Modifier::ProtectedSet(self.expect_any_keyword()?),
            Some(T!["public(set)"]) => Modifier::PublicSet(self.expect_any_keyword()?),
            _ => return Ok(None),
        }))
    }
}

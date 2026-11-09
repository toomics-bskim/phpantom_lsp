//! PHP 5 binds offsets inside unbraced indirect names. Lower those forms
//! to the existing expression selectors, retaining source spans. Empty
//! brace spans mark implicit selectors; formatting makes the braces explicit.

use mago_allocator::prelude::*;
use mago_php_version::PHPVersion;
use mago_span::{HasSpan, Span};

use crate::cst::cst::*;
use crate::parser::Parser;

impl<'arena, A: Arena> Parser<'_, 'arena, A> {
    pub(crate) fn legacy_offset(&self, access: &ArrayAccess<'arena>) -> Option<Expression<'arena>> {
        if self.php_version >= PHPVersion::PHP70 {
            return None;
        }
        let offset = |array| {
            self.arena.alloc(Expression::ArrayAccess(ArrayAccess {
                array,
                ..access.clone()
            }))
        };
        match access.array {
            Expression::Variable(variable) => {
                let indirect = self.legacy_variable_offset(variable, access)?;
                Some(Expression::Variable(Variable::Indirect(indirect)))
            }
            Expression::Access(Access::Property(property)) => {
                let name = match &property.property {
                    ClassLikeMemberSelector::Variable(variable) => {
                        self.arena.alloc(Expression::Variable(variable.clone()))
                    }
                    ClassLikeMemberSelector::Expression(selector) if empty(selector.left_brace) => selector.expression,
                    _ => return None,
                };
                let name = match name {
                    Expression::Variable(variable) => self
                        .legacy_variable_offset(variable, access)
                        .map(|indirect| {
                            self.arena.alloc(Expression::Variable(Variable::Indirect(indirect))) as &Expression<'arena>
                        })
                        .unwrap_or_else(|| offset(name)),
                    _ => offset(name),
                };
                Some(Expression::Access(Access::Property(PropertyAccess {
                    property: implicit_selector(name),
                    ..property.clone()
                })))
            }
            Expression::Access(Access::StaticProperty(property)) => {
                let indirect = self.legacy_variable_offset(&property.property, access)?;
                Some(Expression::Access(Access::StaticProperty(StaticPropertyAccess {
                    property: Variable::Indirect(indirect),
                    ..property.clone()
                })))
            }
            _ => None,
        }
    }

    fn legacy_variable_offset(
        &self,
        variable: &Variable<'arena>,
        access: &ArrayAccess<'arena>,
    ) -> Option<IndirectVariable<'arena>> {
        if !matches!(variable, Variable::Nested(_))
            && !matches!(variable, Variable::Indirect(indirect) if implicit_variable(indirect))
        {
            return None;
        }
        let mut dollars = Vec::new_in(self.arena);
        let mut current = variable;
        let name = loop {
            match current {
                Variable::Nested(nested) => {
                    dollars.push(nested.dollar);
                    current = nested.variable;
                }
                Variable::Indirect(indirect) if implicit_variable(indirect) => {
                    dollars.push(indirect.dollar_left_brace);
                    match indirect.expression {
                        Expression::Variable(variable) => current = variable,
                        expression => break expression,
                    }
                }
                _ => break self.arena.alloc(Expression::Variable(current.clone())),
            }
        };
        let mut expression: &Expression<'arena> = self.arena.alloc(Expression::ArrayAccess(ArrayAccess {
            array: name,
            ..access.clone()
        }));
        while let Some(dollar) = dollars.pop() {
            let indirect = IndirectVariable {
                dollar_left_brace: dollar,
                expression,
                right_brace: Span::new(
                    access.right_bracket.file_id,
                    access.right_bracket.end,
                    access.right_bracket.end,
                ),
            };
            if dollars.is_empty() {
                return Some(indirect);
            }
            expression = self.arena.alloc(Expression::Variable(Variable::Indirect(indirect)));
        }
        None
    }

    pub(crate) fn indirect_call(
        &self,
        function: &'arena Expression<'arena>,
        argument_list: ArgumentList<'arena>,
    ) -> Expression<'arena> {
        if self.php_version < PHPVersion::PHP70 {
            if let Expression::Access(Access::Property(property)) = function
                && let ClassLikeMemberSelector::Expression(selector) = &property.property
                && empty(selector.left_brace)
            {
                return Expression::Call(Call::Method(MethodCall {
                    object: property.object,
                    arrow: property.arrow,
                    method: property.property.clone(),
                    argument_list,
                }));
            }
            if let Expression::Access(Access::StaticProperty(property)) = function
                && let Variable::Indirect(variable) = &property.property
                && implicit_variable(variable)
            {
                return Expression::Call(Call::StaticMethod(StaticMethodCall {
                    class: property.class,
                    double_colon: property.double_colon,
                    method: ClassLikeMemberSelector::Variable(property.property.clone()),
                    argument_list,
                }));
            }
            // `Foo::$names[$key]()` calls a method selected from the local
            // variable in PHP 5, rather than a callable in a static property.
            let mut offsets = Vec::new_in(self.arena);
            let mut base = function;
            while let Expression::ArrayAccess(access) = base {
                offsets.push(access);
                base = access.array;
            }
            if !offsets.is_empty()
                && let Expression::Access(Access::StaticProperty(property)) = base
            {
                let mut name: &Expression<'arena> = self.arena.alloc(Expression::Variable(property.property.clone()));
                for offset in offsets.into_iter().rev() {
                    name = self.arena.alloc(Expression::ArrayAccess(ArrayAccess {
                        array: name,
                        ..offset.clone()
                    }));
                }
                return Expression::Call(Call::StaticMethod(StaticMethodCall {
                    class: property.class,
                    double_colon: property.double_colon,
                    method: implicit_selector(name),
                    argument_list,
                }));
            }
        }
        Expression::Call(Call::Function(FunctionCall {
            function,
            argument_list,
        }))
    }
}

fn empty(span: Span) -> bool {
    span.start == span.end
}

fn implicit_variable(variable: &IndirectVariable<'_>) -> bool {
    variable.dollar_left_brace.end.offset - variable.dollar_left_brace.start.offset == 1
}

fn implicit_selector<'arena>(expression: &'arena Expression<'arena>) -> ClassLikeMemberSelector<'arena> {
    let span = expression.span();
    ClassLikeMemberSelector::Expression(ClassLikeMemberExpressionSelector {
        left_brace: Span::new(span.file_id, span.start, span.start),
        expression,
        right_brace: Span::new(span.file_id, span.end, span.end),
    })
}

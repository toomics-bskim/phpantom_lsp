//! Source-language capabilities, independent of symbol and PHPDoc types.
//!
//! Walk the shared source CST once. Version boundaries follow PHP's migration
//! guides; declarations that used to name classes are deliberately left to
//! native type resolution rather than diagnosed as unavailable syntax.

use mago_php_version::PHPVersion;
use mago_span::{HasSpan, Span};
use mago_syntax::cst::*;
use mago_syntax::walker::{self, Walker};
use tower_lsp_server::gen_lsp_types::*;

use crate::Backend;
use crate::parser::{SourceVersionGuard, with_parsed_program};

use super::helpers::make_diagnostic;

impl Backend {
    /// Report language constructs introduced after the file's PHP target.
    pub fn collect_unsupported_syntax_diagnostics(
        &self,
        uri: &str,
        content: &str,
        out: &mut Vec<Diagnostic>,
    ) {
        let version = self.source_php_version(uri);
        // All capabilities checked here are available at the default target.
        if version >= PHPVersion::PHP85 {
            return;
        }
        let _version = SourceVersionGuard::new(version);
        with_parsed_program(content, "unsupported_syntax", |program, source| {
            let checker = Capabilities;
            let mut context = Context {
                backend: self,
                uri,
                source,
                version,
                out,
                functions: Vec::new(),
                returns: Vec::new(),
            };
            checker.walk_program(program, &mut context);
        });
    }
}

struct Context<'a> {
    backend: &'a Backend,
    uri: &'a str,
    source: &'a str,
    version: PHPVersion,
    out: &'a mut Vec<Diagnostic>,
    functions: Vec<GeneratorScope>,
    // Reuse return storage across functions instead of allocating per body.
    returns: Vec<Span>,
}

struct GeneratorScope {
    yields: bool,
    return_start: usize,
}

impl Context<'_> {
    fn require(&mut self, span: Span, major: u32, minor: u32, feature: &str) {
        if self.version >= PHPVersion::new(major, minor, 0) {
            return;
        }
        if let Some(range) = self.backend.offset_range_to_lsp_range(
            self.uri,
            self.source,
            span.start.offset as usize,
            span.end.offset as usize,
        ) {
            self.out.push(make_diagnostic(
                range,
                DiagnosticSeverity::Error,
                "unsupported_syntax",
                format!(
                    "{feature}: requires PHP {major}.{minor}; the selected PHP version is {}.",
                    self.version
                ),
            ));
        }
    }

    fn enter_function(&mut self) {
        if self.version < PHPVersion::PHP70 {
            self.functions.push(GeneratorScope {
                yields: false,
                return_start: self.returns.len(),
            });
        }
    }

    fn leave_function(&mut self) {
        if let Some(scope) = self.functions.pop() {
            if scope.yields {
                for index in scope.return_start..self.returns.len() {
                    self.require(
                        self.returns[index],
                        7,
                        0,
                        "Returning a value from a generator",
                    );
                }
            }
            self.returns.truncate(scope.return_start);
        }
    }
}

struct Capabilities;

macro_rules! capability {
    ($method:ident, $node:ident, $major:literal, $minor:literal, $label:literal) => {
        fn $method(&self, node: &'ast $node<'arena>, context: &mut Context<'_>) {
            context.require(node.span(), $major, $minor, $label);
        }
    };
}

macro_rules! function_scope {
    ($enter:ident, $leave:ident, $node:ident) => {
        fn $enter(&self, _: &'ast $node<'arena>, context: &mut Context<'_>) {
            context.enter_function();
        }
        fn $leave(&self, _: &'ast $node<'arena>, context: &mut Context<'_>) {
            context.leave_function();
        }
    };
}

impl<'ast, 'arena> Walker<'ast, 'arena, Context<'_>> for Capabilities {
    capability!(
        walk_in_function_like_return_type_hint,
        FunctionLikeReturnTypeHint,
        7,
        0,
        "Return type declarations"
    );
    capability!(
        walk_in_anonymous_class,
        AnonymousClass,
        7,
        0,
        "Anonymous classes"
    );
    capability!(walk_in_yield_from, YieldFrom, 7, 0, "Generator delegation");
    capability!(
        walk_in_named_argument,
        NamedArgument,
        8,
        0,
        "Named arguments"
    );
    capability!(
        walk_in_null_safe_method_call,
        NullSafeMethodCall,
        8,
        0,
        "Nullsafe calls"
    );
    capability!(
        walk_in_null_safe_property_access,
        NullSafePropertyAccess,
        8,
        0,
        "Nullsafe access"
    );
    capability!(
        walk_in_property_hook_list,
        PropertyHookList,
        8,
        4,
        "Property hooks"
    );
    capability!(
        walk_in_variadic_array_element,
        VariadicArrayElement,
        7,
        4,
        "Array unpacking"
    );
    capability!(walk_in_pipe, Pipe, 8, 5, "The pipe operator");
    capability!(walk_in_enum, Enum, 8, 1, "Enumerations");
    capability!(walk_in_match, Match, 8, 0, "Match expressions");
    capability!(walk_in_throw, Throw, 8, 0, "Throw expressions");

    function_scope!(walk_in_function, walk_out_function, Function);
    function_scope!(walk_in_method, walk_out_method, Method);
    function_scope!(walk_in_closure, walk_out_closure, Closure);

    fn walk_in_arrow_function(&self, node: &'ast ArrowFunction<'arena>, context: &mut Context<'_>) {
        context.require(node.r#fn.span(), 7, 4, "Arrow functions");
        context.enter_function();
    }

    fn walk_out_arrow_function(&self, _: &'ast ArrowFunction<'arena>, context: &mut Context<'_>) {
        context.leave_function();
    }

    fn walk_statement_expression(
        &self,
        node: &'ast ExpressionStatement<'arena>,
        context: &mut Context<'_>,
    ) {
        if let Expression::Throw(throw) = node.expression {
            // A standalone throw was a statement long before expression throws.
            self.walk_expression(throw.exception, context);
        } else {
            walker::walk_statement_expression(self, node, context);
        }
    }

    fn walk_in_yield(&self, _: &'ast Yield<'arena>, context: &mut Context<'_>) {
        if let Some(scope) = context.functions.last_mut() {
            scope.yields = true;
        }
    }

    fn walk_in_return(&self, node: &'ast Return<'arena>, context: &mut Context<'_>) {
        if node.value.is_some() && !context.functions.is_empty() {
            context.returns.push(node.span());
        }
    }

    fn walk_in_function_like_parameter(
        &self,
        node: &'ast FunctionLikeParameter<'arena>,
        context: &mut Context<'_>,
    ) {
        if let Some(span) = node.ellipsis {
            context.require(span, 5, 6, "Variadic parameters");
        }
        if node.is_promoted_property() {
            context.require(node.span(), 8, 0, "Constructor property promotion");
        }
    }

    fn walk_in_positional_argument(
        &self,
        node: &'ast PositionalArgument<'arena>,
        context: &mut Context<'_>,
    ) {
        if let Some(span) = node.ellipsis {
            context.require(span, 5, 6, "Argument unpacking");
        }
    }

    fn walk_in_binary_operator(
        &self,
        node: &'ast BinaryOperator<'arena>,
        context: &mut Context<'_>,
    ) {
        match node {
            BinaryOperator::Exponentiation(span) => context.require(*span, 5, 6, "Exponentiation"),
            BinaryOperator::NullCoalesce(span) => context.require(*span, 7, 0, "Null coalescing"),
            BinaryOperator::Spaceship(span) => {
                context.require(*span, 7, 0, "The spaceship operator")
            }
            _ => {}
        }
    }

    fn walk_in_binary(&self, node: &'ast Binary<'arena>, context: &mut Context<'_>) {
        if matches!(node.operator, BinaryOperator::Instanceof(_))
            && matches!(node.lhs, Expression::Literal(_))
        {
            context.require(node.lhs.span(), 7, 3, "Literal operands of instanceof");
        }
    }

    fn walk_in_class_constant_access(
        &self,
        node: &'ast ClassConstantAccess<'arena>,
        context: &mut Context<'_>,
    ) {
        if matches!(node.constant, ClassLikeConstantSelector::Expression(_)) {
            context.require(node.constant.span(), 8, 3, "Dynamic class constant access");
        }
    }

    fn walk_in_trait(&self, node: &'ast Trait<'arena>, context: &mut Context<'_>) {
        for member in &node.members {
            if let ClassLikeMember::Constant(constant) = member {
                context.require(constant.r#const.span(), 8, 2, "Trait constants");
            }
        }
    }

    fn walk_in_expression(&self, node: &'ast Expression<'arena>, context: &mut Context<'_>) {
        let subject = match node {
            Expression::Call(Call::Method(call)) => call.object,
            Expression::Call(Call::NullSafeMethod(call)) => call.object,
            Expression::Call(Call::Function(call)) => {
                if matches!(
                    call.function,
                    Expression::Parenthesized(_) | Expression::Call(_)
                ) {
                    context.require(call.function.span(), 7, 0, "Calling arbitrary expressions");
                }
                call.function
            }
            Expression::Call(Call::StaticMethod(call)) => call.class,
            Expression::Access(Access::Property(access)) => access.object,
            Expression::Access(Access::NullSafeProperty(access)) => access.object,
            Expression::Access(Access::StaticProperty(access)) => access.class,
            Expression::Access(Access::ClassConstant(access)) => access.class,
            Expression::ArrayAccess(access) => access.array,
            _ => return,
        };
        if let Expression::Instantiation(new) = subject {
            context.require(
                new.new.span(),
                8,
                4,
                "Dereferencing new without parentheses",
            );
        }
    }

    fn walk_in_assignment_operator(
        &self,
        node: &'ast AssignmentOperator,
        context: &mut Context<'_>,
    ) {
        match node {
            AssignmentOperator::Exponentiation(span) => {
                context.require(*span, 5, 6, "Exponentiation assignment")
            }
            AssignmentOperator::Coalesce(span) => {
                context.require(*span, 7, 4, "Null coalescing assignment")
            }
            _ => {}
        }
    }

    fn walk_in_hint(&self, node: &'ast Hint<'arena>, context: &mut Context<'_>) {
        match node {
            Hint::Nullable(_) => context.require(node.span(), 7, 1, "Nullable types"),
            Hint::Union(_) => context.require(node.span(), 8, 0, "Union types"),
            Hint::Intersection(_) => context.require(node.span(), 8, 1, "Intersection types"),
            Hint::Parenthesized(_) => {
                context.require(node.span(), 8, 2, "Disjunctive normal form types")
            }
            Hint::Static(_) => context.require(node.span(), 8, 0, "Static return types"),
            _ => {}
        }
    }

    fn walk_try_catch_clause(&self, node: &'ast TryCatchClause<'arena>, context: &mut Context<'_>) {
        // Catch alternatives precede union type declarations by three releases.
        if matches!(node.hint, Hint::Union(_)) {
            context.require(node.hint.span(), 7, 1, "Multi-catch");
        }
        if node.variable.is_none() {
            context.require(node.r#catch.span(), 8, 0, "Non-capturing catches");
        }
        self.walk_block(&node.block, context);
    }

    fn walk_in_plain_property(&self, node: &'ast PlainProperty<'arena>, context: &mut Context<'_>) {
        if let Some(hint) = &node.hint {
            context.require(hint.span(), 7, 4, "Typed properties");
        }
        for modifier in &node.modifiers {
            if matches!(modifier, Modifier::Final(_) | Modifier::Abstract(_)) {
                context.require(modifier.span(), 8, 4, "Final and abstract properties");
            }
        }
    }

    fn walk_in_class_like_constant(
        &self,
        node: &'ast ClassLikeConstant<'arena>,
        context: &mut Context<'_>,
    ) {
        if let Some(hint) = &node.hint {
            context.require(hint.span(), 8, 3, "Typed class constants");
        }
        for modifier in &node.modifiers {
            if modifier.is_visibility() {
                context.require(modifier.span(), 7, 1, "Class constant visibility");
            } else if matches!(modifier, Modifier::Final(_)) {
                context.require(modifier.span(), 8, 1, "Final class constants");
            }
        }
    }

    fn walk_in_modifier(&self, node: &'ast Modifier<'arena>, context: &mut Context<'_>) {
        if node.is_write_visibility() {
            context.require(node.span(), 8, 4, "Asymmetric visibility");
        } else if matches!(node, Modifier::Readonly(_)) {
            context.require(node.span(), 8, 1, "Readonly modifiers");
        }
    }

    fn walk_in_class(&self, node: &'ast Class<'arena>, context: &mut Context<'_>) {
        for modifier in &node.modifiers {
            if matches!(modifier, Modifier::Readonly(_)) {
                context.require(modifier.span(), 8, 2, "Readonly classes");
            }
        }
    }

    fn walk_in_use_items(&self, node: &'ast UseItems<'arena>, context: &mut Context<'_>) {
        match node {
            UseItems::TypedSequence(items) => {
                context.require(items.r#type.span(), 5, 6, "Function and constant imports")
            }
            UseItems::TypedList(_) | UseItems::MixedList(_) => {
                context.require(node.span(), 7, 0, "Group imports")
            }
            _ => {}
        }
    }

    fn walk_in_mixed_use_item_list(
        &self,
        node: &'ast MixedUseItemList<'arena>,
        context: &mut Context<'_>,
    ) {
        if let Some(token) = node.items.tokens.last()
            && node.items.tokens.len() == node.items.nodes.len()
        {
            context.require(
                token.span_for(node.span().file_id),
                7,
                2,
                "Trailing commas in group imports",
            );
        }
    }

    fn walk_in_typed_use_item_list(
        &self,
        node: &'ast TypedUseItemList<'arena>,
        context: &mut Context<'_>,
    ) {
        if let Some(token) = node.items.tokens.last()
            && node.items.tokens.len() == node.items.nodes.len()
        {
            context.require(
                token.span_for(node.span().file_id),
                7,
                2,
                "Trailing commas in group imports",
            );
        }
    }

    fn walk_in_argument_list(&self, node: &'ast ArgumentList<'arena>, context: &mut Context<'_>) {
        if let Some(token) = node.arguments.tokens.last()
            && node.arguments.tokens.len() == node.arguments.nodes.len()
        {
            context.require(
                token.span_for(node.span().file_id),
                7,
                3,
                "Trailing commas in calls",
            );
        }
    }

    fn walk_in_function_like_parameter_list(
        &self,
        node: &'ast FunctionLikeParameterList<'arena>,
        context: &mut Context<'_>,
    ) {
        if let Some(token) = node.parameters.tokens.last()
            && node.parameters.tokens.len() == node.parameters.nodes.len()
        {
            context.require(
                token.span_for(node.span().file_id),
                8,
                0,
                "Trailing commas in parameter lists",
            );
        }
    }

    fn walk_in_closure_use_clause(
        &self,
        node: &'ast ClosureUseClause<'arena>,
        context: &mut Context<'_>,
    ) {
        if let Some(token) = node.variables.tokens.last()
            && node.variables.tokens.len() == node.variables.nodes.len()
        {
            context.require(
                token.span_for(node.span().file_id),
                8,
                0,
                "Trailing commas in closure use lists",
            );
        }
    }

    fn walk_in_partial_argument_list(
        &self,
        node: &'ast PartialArgumentList<'arena>,
        context: &mut Context<'_>,
    ) {
        context.require(node.span(), 8, 1, "First-class callable syntax");
    }

    fn walk_in_assignment(&self, node: &'ast Assignment<'arena>, context: &mut Context<'_>) {
        destructuring(node.lhs, context);
    }

    fn walk_in_foreach_target(&self, node: &'ast ForeachTarget<'arena>, context: &mut Context<'_>) {
        let value = match node {
            ForeachTarget::Value(target) => target.value,
            ForeachTarget::KeyValue(target) => target.value,
        };
        destructuring(value, context);
    }

    fn walk_in_document_string(
        &self,
        node: &'ast DocumentString<'arena>,
        context: &mut Context<'_>,
    ) {
        let Some(tail) = context.source.get(node.close.end.offset as usize..) else {
            return;
        };
        let tail = tail.strip_prefix(';').unwrap_or(tail);
        if !matches!(node.indentation, DocumentIndentation::None)
            || (!tail.is_empty() && !tail.starts_with(['\r', '\n']))
        {
            context.require(node.close, 7, 3, "Flexible heredoc and nowdoc syntax");
        }
    }

    fn walk_in_literal_integer(
        &self,
        node: &'ast LiteralInteger<'arena>,
        context: &mut Context<'_>,
    ) {
        numeric_literal(node.span(), context);
    }

    fn walk_in_literal_float(&self, node: &'ast LiteralFloat<'arena>, context: &mut Context<'_>) {
        numeric_literal(node.span(), context);
    }

    fn walk_in_constant_item(&self, node: &'ast ConstantItem<'arena>, context: &mut Context<'_>) {
        constant_initializer(node.value, true, context);
    }

    fn walk_in_class_like_constant_item(
        &self,
        node: &'ast ClassLikeConstantItem<'arena>,
        context: &mut Context<'_>,
    ) {
        constant_initializer(node.value, true, context);
    }

    fn walk_in_property_concrete_item(
        &self,
        node: &'ast PropertyConcreteItem<'arena>,
        context: &mut Context<'_>,
    ) {
        constant_initializer(node.value, false, context);
    }

    fn walk_in_function_like_parameter_default_value(
        &self,
        node: &'ast FunctionLikeParameterDefaultValue<'arena>,
        context: &mut Context<'_>,
    ) {
        constant_initializer(node.value, false, context);
        if matches!(node.value, Expression::Instantiation(_)) {
            context.require(node.value.span(), 8, 1, "New expressions in initializers");
        }
    }

    fn walk_in_static_concrete_item(
        &self,
        node: &'ast StaticConcreteItem<'arena>,
        context: &mut Context<'_>,
    ) {
        constant_initializer(node.value, false, context);
    }
}

fn numeric_literal(span: Span, context: &mut Context<'_>) {
    let raw = &context.source[span.start.offset as usize..span.end.offset as usize];
    if raw.contains('_') {
        context.require(span, 7, 4, "Numeric literal separators");
    }
    if raw.starts_with("0o") || raw.starts_with("0O") {
        context.require(span, 8, 1, "Explicit octal notation");
    }
}

fn destructuring(expression: &Expression<'_>, context: &mut Context<'_>) {
    let elements = match expression {
        Expression::Array(array) => {
            context.require(array.left_bracket, 7, 1, "Short array destructuring");
            &array.elements
        }
        Expression::List(list) => &list.elements,
        Expression::UnaryPrefix(prefix)
            if matches!(prefix.operator, UnaryPrefixOperator::Reference(_)) =>
        {
            destructuring(prefix.operand, context);
            return;
        }
        _ => return,
    };
    for element in elements {
        let value = match element {
            ArrayElement::KeyValue(item) => {
                context.require(item.double_arrow, 7, 1, "Keyed destructuring");
                item.value
            }
            ArrayElement::Value(item) => item.value,
            _ => continue,
        };
        if let Expression::UnaryPrefix(prefix) = value
            && matches!(prefix.operator, UnaryPrefixOperator::Reference(_))
        {
            context.require(prefix.operator.span(), 7, 3, "References in destructuring");
        }
        destructuring(value, context);
    }
}

fn constant_initializer(expression: &Expression<'_>, constant: bool, context: &mut Context<'_>) {
    if context.version >= PHPVersion::new(5, 6, 0) {
        return;
    }
    if constant
        && matches!(
            expression,
            Expression::Array(_) | Expression::LegacyArray(_)
        )
    {
        context.require(expression.span(), 5, 6, "Constant arrays");
    }
    Initializer.walk_expression(expression, context);
}

struct Initializer;

impl<'ast, 'arena> Walker<'ast, 'arena, Context<'_>> for Initializer {
    fn walk_expression(&self, node: &'ast Expression<'arena>, context: &mut Context<'_>) {
        let expanded = matches!(
            node,
            Expression::Binary(_)
                | Expression::Conditional(_)
                | Expression::Parenthesized(_)
                | Expression::ArrayAccess(_)
        ) || matches!(node, Expression::UnaryPrefix(prefix) if matches!(prefix.operator, UnaryPrefixOperator::Not(_) | UnaryPrefixOperator::BitwiseNot(_)));
        if expanded {
            context.require(node.span(), 5, 6, "Constant expressions");
        } else {
            walker::walk_expression(self, node, context);
        }
    }
}

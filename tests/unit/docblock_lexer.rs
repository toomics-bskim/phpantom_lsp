use mago_allocator::arena::LocalArena;
use mago_database::file::FileId;
use mago_phpdoc_syntax::PHPDocParser;
use mago_phpdoc_syntax::cst::{TagValue, TriviaKind, r#type::Type};
use mago_span::{Position, Span};

#[test]
fn phpdoc_line_comments_preserve_boundaries_and_source_spans() {
    let cases = [
        ("/** @var int // comment\n */", "// comment", true),
        ("/** @var int // comment\r\n */", "// comment", true),
        ("/** @var int // comment\r */", "// comment", true),
        ("/** @var int // 설명 😀\n */", "// 설명 😀", true),
        ("/** @var int // comment */", "// comment ", true),
        ("/** @var int // comment ***/", "// comment ", true),
        ("/** @var int // comment *\n */", "// comment *", true),
        ("/** @var int // comment", "// comment", false),
        ("/** @var int //", "//", false),
        ("/** @var int //*/", "//", true),
    ];
    for (source, expected_comment, closed) in cases {
        let arena = LocalArena::new();
        let offset = 73;
        let span = Span::new(
            FileId::zero(),
            Position::new(offset),
            Position::new(offset + source.len() as u32),
        );
        let document = PHPDocParser::parse_with_span(&arena, source.as_bytes(), span);
        let comments: Vec<_> = document
            .trivia
            .iter()
            .filter(|trivia| trivia.kind == TriviaKind::LineComment)
            .collect();
        assert_eq!(comments.len(), 1, "{source:?}");
        let comment = comments[0];
        assert_eq!(comment.value, expected_comment.as_bytes(), "{source:?}");
        let start = (comment.span.start.offset - offset) as usize;
        let end = (comment.span.end.offset - offset) as usize;
        assert_eq!(&source[start..end], expected_comment);
        assert_eq!(
            document
                .trivia
                .iter()
                .any(|trivia| trivia.kind == TriviaKind::ClosingMarker),
            closed,
            "{source:?}"
        );
        assert_eq!(document.tags().count(), 1, "{source:?}");
    }
}

#[test]
fn phpdoc_comments_do_not_consume_following_fields_or_tags() {
    let source = "/**\r\n * @psalm-type Config = array{\r\n *   url: 'https://example.test', // URL\r\n *   pattern: '//literal', // pattern\r\n *   enabled?: bool, // Default: true\r\n * }\r\n * @return Config\r\n */";
    let arena = LocalArena::new();
    let document = PHPDocParser::parse(&arena, FileId::zero(), source.as_bytes());
    assert!(document.errors.is_empty(), "{:?}", document.errors);
    let comments: Vec<_> = document
        .trivia
        .iter()
        .filter(|trivia| trivia.kind == TriviaKind::LineComment)
        .map(|trivia| trivia.value)
        .collect();
    assert_eq!(
        comments,
        [b"// URL".as_slice(), b"// pattern", b"// Default: true"]
    );
    assert_eq!(document.tags().count(), 2);
    let TagValue::TypeAlias(alias) = &document.tags().next().unwrap().value else {
        panic!("Expected a type alias");
    };
    let Type::Shape(shape) = alias.r#type else {
        panic!("Expected an array shape");
    };
    assert_eq!(shape.fields.len(), 3);
    let ty = phpantom_lsp::php_type::PhpType::parse(
        "array{url: 'https://example.test', // URL\n pattern: '//literal', // pattern\n enabled?: bool, // Default: true\n}",
    );
    assert_eq!(
        ty,
        phpantom_lsp::php_type::PhpType::parse(
            "array{url: 'https://example.test', pattern: '//literal', enabled?: bool}"
        )
    );
}

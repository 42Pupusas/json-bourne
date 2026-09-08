//! Constructor and `MAX_DEPTH` contract: audit 2026-09 F1.
//!
//! `Parser::new` / `Lexer::new` must work with no type annotation at all —
//! the struct's `= DEFAULT_MAX_DEPTH` const default is otherwise inert
//! (it does not flow through the generic impl block), which used to make
//! the bare constructors fail to compile with E0284. Custom depths go
//! through `with_depth`.

use json_bourne::{DEFAULT_MAX_DEPTH, ErrorKind, Event, Lexer, Parser};

/// The annotation-free forms compile and behave as the default-depth
/// constructors. Before F1 both of these were E0284.
#[test]
fn new_compiles_without_annotation() {
    let mut p = Parser::new(b"[1,{\"k\":true}]");
    assert!(matches!(p.next_event(), Ok(Some(Event::StartArray))));
    let mut lex = Lexer::new(b"\"ok\"");
    assert!(matches!(lex.read_value(), Ok(Event::String(_))));
    let mut lex = Lexer::new(b"[1]");
    assert!(lex.skip_value().is_ok());
}

/// Builds `[` × n + `]` × n.
fn nested(n: usize) -> Vec<u8> {
    let mut input = vec![b'['; n];
    input.extend(std::iter::repeat_n(b']', n));
    input
}

/// The constructor docs and the struct docs all claim the default depth
/// is `DEFAULT_MAX_DEPTH`; this pins the claim by behaviour. `[` is one
/// frame per byte, so exactly `DEFAULT_MAX_DEPTH` opens must parse.
#[test]
fn new_uses_default_max_depth() {
    let depth = DEFAULT_MAX_DEPTH;
    let input = nested(depth);

    let mut p = Parser::new(&input);
    loop {
        match p.next_event() {
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(e) => panic!("default depth rejected its own limit: {e}"),
        }
    }

    let input = nested(depth + 1);
    let mut p = Parser::new(&input);
    let mut err = None;
    while err.is_none() {
        match p.next_event() {
            Ok(Some(_)) => {}
            Ok(None) => panic!("over-limit nesting parsed"),
            Err(e) => err = Some(e),
        }
    }
    assert_eq!(err.unwrap().kind, ErrorKind::DepthLimitExceeded);
}

/// `Lexer::new` inherits the same default by delegation.
#[test]
fn lexer_new_uses_default_max_depth() {
    let input = nested(DEFAULT_MAX_DEPTH);
    let mut lex = Lexer::new(&input);
    lex.skip_value().expect("default depth parses");

    let input = nested(DEFAULT_MAX_DEPTH + 1);
    let mut lex = Lexer::new(&input);
    assert_eq!(
        lex.skip_value().unwrap_err().kind,
        ErrorKind::DepthLimitExceeded,
    );
}

/// Custom depths keep working, through the dedicated constructor, and
/// the limit is enforced at exactly the chosen depth.
#[test]
fn with_depth_builds_custom_limit() {
    let at_limit = nested(4);
    let mut lex: Lexer<'_, 4> = Lexer::with_depth(&at_limit);
    lex.skip_value().expect("depth 4 input at limit 4 parses");

    let over_limit = nested(5);
    let mut lex: Lexer<'_, 4> = Lexer::with_depth(&over_limit);
    assert_eq!(
        lex.skip_value().unwrap_err().kind,
        ErrorKind::DepthLimitExceeded,
    );

    let parser_over = nested(5);
    let mut p: Parser<'_, 4> = Parser::with_depth(&parser_over);
    let mut err = None;
    while err.is_none() {
        match p.next_event() {
            Ok(Some(_)) => {}
            Ok(None) => panic!("over-limit nesting parsed"),
            Err(e) => err = Some(e),
        }
    }
    assert_eq!(err.unwrap().kind, ErrorKind::DepthLimitExceeded);
}

/// The `try_new` pair keeps its `Result` contract on both constructors,
/// at default depth.
#[test]
fn try_new_still_returns_result() {
    let lex = Lexer::try_new(b"{\"k\":1}").unwrap();
    assert_eq!(lex.input().len(), 7);
    let mut p = Parser::try_new(b"{\"k\":1}").unwrap();
    assert!(matches!(p.next_event(), Ok(Some(Event::StartObject))));
}

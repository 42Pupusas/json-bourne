//! Pathological-input regression tests.
//!
//! Inputs that try to overflow the stack, balloon allocations, or
//! trigger quadratic behavior. Each test asserts the parse either
//! completes within reasonable time or fails with a specific bounded
//! error (typically `DepthLimitExceeded`). The point is that nothing
//! aborts the process, panics, or runs arbitrarily long.

use bourne::{ErrorKind, parse_str};
use bourne_core::DEFAULT_MAX_DEPTH;

fn opens(n: usize, ch: char) -> String {
    let mut s = String::with_capacity(n);
    for _ in 0..n {
        s.push(ch);
    }
    s
}

#[test]
fn deeply_nested_arrays_within_limit_succeed() {
    let depth = DEFAULT_MAX_DEPTH - 1;
    let input = format!("{}{}", opens(depth, '['), opens(depth, ']'));
    // Parses as Vec<Vec<...>>; we just verify it lexes & balances.
    // Use Vec<u8> at the leaf to give the type a concrete bottom.
    let bytes = input.into_bytes();
    let mut p: bourne_core::Parser<'_> = bourne_core::Parser::new(&bytes);
    while let Some(_ev) = p.next_event().expect("parses within depth limit") {}
}

#[test]
fn deeply_nested_arrays_above_limit_errors() {
    let depth = DEFAULT_MAX_DEPTH + 64;
    let input = format!("{}{}", opens(depth, '['), opens(depth, ']'));
    let bytes = input.into_bytes();
    let mut p: bourne_core::Parser<'_> = bourne_core::Parser::new(&bytes);
    let mut hit_limit = false;
    loop {
        match p.next_event() {
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(e) => {
                assert_eq!(
                    e.kind,
                    ErrorKind::DepthLimitExceeded,
                    "want DepthLimitExceeded, got {:?}",
                    e.kind,
                );
                hit_limit = true;
                break;
            }
        }
    }
    assert!(hit_limit, "should have hit DepthLimitExceeded");
}

#[test]
fn deeply_nested_objects_above_limit_errors() {
    // {"a":{"a":{...}...}} chains. Build the prefix, then close.
    let depth = DEFAULT_MAX_DEPTH + 32;
    let mut input = String::new();
    for _ in 0..depth {
        input.push_str(r#"{"a":"#);
    }
    input.push('1');
    for _ in 0..depth {
        input.push('}');
    }
    let bytes = input.into_bytes();
    let mut p: bourne_core::Parser<'_> = bourne_core::Parser::new(&bytes);
    let mut hit_limit = false;
    loop {
        match p.next_event() {
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(e) => {
                assert_eq!(e.kind, ErrorKind::DepthLimitExceeded);
                hit_limit = true;
                break;
            }
        }
    }
    assert!(hit_limit);
}

#[test]
fn alternating_array_object_above_limit_errors() {
    let depth = DEFAULT_MAX_DEPTH + 16;
    let mut input = String::new();
    for i in 0..depth {
        input.push(if i % 2 == 0 { '[' } else { '{' });
        if i % 2 == 1 {
            input.push_str(r#""k":"#);
        }
    }
    input.push('1');
    for i in (0..depth).rev() {
        input.push(if i % 2 == 0 { ']' } else { '}' });
    }
    let bytes = input.into_bytes();
    let mut p: bourne_core::Parser<'_> = bourne_core::Parser::new(&bytes);
    let mut hit_limit = false;
    loop {
        match p.next_event() {
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(e) => {
                assert_eq!(e.kind, ErrorKind::DepthLimitExceeded);
                hit_limit = true;
                break;
            }
        }
    }
    assert!(hit_limit);
}

#[test]
fn very_long_string_parses() {
    // 1 MiB of ASCII inside a JSON string. Validates the SSE2 scan
    // doesn't degrade (timing) and the deferred validation doesn't
    // explode allocations.
    let body = "a".repeat(1024 * 1024);
    let json = format!(r#""{body}""#);
    let s: String = parse_str(&json).expect("long string parses");
    assert_eq!(s.len(), body.len());
}

#[test]
fn very_long_escaped_string_parses() {
    // 100 KiB of `\n` escapes — exercises the escape decoder loop, not
    // just the SSE2 fast path.
    let body = r"\n".repeat(100 * 1024);
    let json = format!(r#""{body}""#);
    let s: String = parse_str(&json).expect("escaped string parses");
    assert_eq!(s.len(), 100 * 1024);
    assert!(s.chars().all(|c| c == '\n'));
}

use bourne::from_json;

from_json! {
    #[bourne(deny_unknown_fields = false)]
    #[derive(Debug)]
    struct Empty {}
}

from_json! {
    #[bourne(deny_unknown_fields = false)]
    #[derive(Debug)]
    struct Wrap {
        id: u32,
    }
}

#[test]
fn many_field_object_parses() {
    use core::fmt::Write as _;
    // 10k unknown fields in lenient mode — validates the unknown-key
    // skip path doesn't have quadratic behavior in field count.
    let mut input = String::from("{");
    for i in 0..10_000 {
        if i > 0 {
            input.push(',');
        }
        write!(&mut input, r#""k{i}":{i}"#).unwrap();
    }
    input.push('}');

    // The lenient struct accepts any object and skips every field.
    let _e: Empty = parse_str(&input).expect("many-field object parses");
}

#[test]
fn many_element_array_parses() {
    let mut input = String::from("[");
    for i in 0..10_000 {
        if i > 0 {
            input.push(',');
        }
        input.push_str(&i.to_string());
    }
    input.push(']');
    let v: Vec<i32> = parse_str(&input).expect("many-element array parses");
    assert_eq!(v.len(), 10_000);
    assert_eq!(v[9_999], 9_999);
}

#[test]
fn many_long_strings_in_array_parses() {
    // Each element is a 1 KiB string; 1k elements = 1 MiB total.
    let elem = "x".repeat(1024);
    let mut input = String::from("[");
    for i in 0..1_000 {
        if i > 0 {
            input.push(',');
        }
        input.push('"');
        input.push_str(&elem);
        input.push('"');
    }
    input.push(']');
    let v: Vec<String> = parse_str(&input).expect("array of strings parses");
    assert_eq!(v.len(), 1_000);
    assert_eq!(v[0].len(), 1024);
}

#[test]
fn skip_value_handles_deep_nesting_within_limit() {
    // Lenient struct skips an unknown deep value. The skip path
    // recurses through the value; verify it tracks the same depth
    // limit as the typed path.
    let depth = DEFAULT_MAX_DEPTH - 5; // leave headroom for the wrapper
    let mut input = String::from(r#"{"id":1,"deep":"#);
    input.push_str(&opens(depth, '['));
    input.push('1');
    input.push_str(&opens(depth, ']'));
    input.push('}');
    let _w: Wrap = parse_str(&input).expect("deep skip within limit parses");
}

#[test]
fn skip_value_rejects_above_limit() {
    let depth = DEFAULT_MAX_DEPTH + 16;
    let mut input = String::from(r#"{"id":1,"deep":"#);
    input.push_str(&opens(depth, '['));
    input.push('1');
    input.push_str(&opens(depth, ']'));
    input.push('}');
    let err = parse_str::<Wrap>(&input).unwrap_err();
    assert_eq!(err.kind, ErrorKind::DepthLimitExceeded);
}

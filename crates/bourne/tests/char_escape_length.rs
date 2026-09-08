//! `char` rejects strings that are not exactly one scalar — including
//! on the escape path, where the decode goes to a 4-byte stack scratch.
//!
//! The scratch used to drop any run that did not fit instead of
//! reporting overflow, so `"\u0041BCDE"` decoded to just `'A'` and the
//! "exactly one scalar" check passed on the truncated value. Only
//! strings whose literal tail happened to *partly* fit (`"\u0041BC"`,
//! wanting bytes 1..3 of a 4-byte buffer) were rejected, which is why
//! the existing cases missed it.

use json_bourne::parse_str;

#[test]
fn escaped_char_followed_by_literal_run_is_rejected() {
    for src in [
        r#""\u0041BCDE""#,
        r#""\u0041BC""#,
        r#""\u0041B""#,
        r#""\nABCDE""#,
        r#""\tXY""#,
        r#""\\ab""#,
    ] {
        assert!(
            parse_str::<char>(src).is_err(),
            "{src} is more than one scalar and must not parse as char"
        );
    }
}

#[test]
fn literal_run_followed_by_escape_is_rejected() {
    for src in [r#""AB\u0043""#, r#""ABCDE\n""#, r#""A\nB""#] {
        assert!(
            parse_str::<char>(src).is_err(),
            "{src} is more than one scalar and must not parse as char"
        );
    }
}

#[test]
fn single_escaped_scalars_still_parse() {
    let cases: &[(&str, char)] = &[
        (r#""\u0041""#, 'A'),
        (r#""\n""#, '\n'),
        (r#""\t""#, '\t'),
        (r#""\\""#, '\\'),
        (r#""\"""#, '"'),
        (r#""\u00e9""#, 'é'),
        (r#""\u20ac""#, '€'),
        // Surrogate pair -> 4 UTF-8 bytes, exactly the scratch capacity.
        (r#""\ud83d\ude00""#, '😀'),
    ];
    for &(src, want) in cases {
        assert_eq!(parse_str::<char>(src).unwrap(), want, "case {src}");
    }
}

#[test]
fn multi_scalar_without_escapes_still_rejected() {
    assert!(parse_str::<char>(r#""AB""#).is_err());
    assert!(parse_str::<char>(r#""""#).is_err());
}

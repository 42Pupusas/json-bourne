//! Exhaustive checks for the compile-time case converter that backs
//! `#[bourne(rename_all = "…")]`. These call the (doc-hidden) internal
//! entry point directly so the converter is validated independently of
//! the macro plumbing that consumes it.

#![cfg(feature = "std")]

use json_bourne::__Casing as Casing;

fn c(src: &str, style: Casing) -> String {
    style.convert(src).as_str().to_owned()
}

#[test]
fn snake_field_to_all_styles() {
    let s = "user_id";
    assert_eq!(c(s, Casing::Lower), "userid");
    assert_eq!(c(s, Casing::Upper), "USERID");
    assert_eq!(c(s, Casing::Pascal), "UserId");
    assert_eq!(c(s, Casing::Camel), "userId");
    assert_eq!(c(s, Casing::Snake), "user_id");
    assert_eq!(c(s, Casing::ScreamingSnake), "USER_ID");
    assert_eq!(c(s, Casing::Kebab), "user-id");
    assert_eq!(c(s, Casing::ScreamingKebab), "USER-ID");
}

#[test]
fn pascal_variant_to_all_styles() {
    let s = "HttpError";
    assert_eq!(c(s, Casing::Lower), "httperror");
    assert_eq!(c(s, Casing::Upper), "HTTPERROR");
    assert_eq!(c(s, Casing::Pascal), "HttpError");
    assert_eq!(c(s, Casing::Camel), "httpError");
    assert_eq!(c(s, Casing::Snake), "http_error");
    assert_eq!(c(s, Casing::ScreamingSnake), "HTTP_ERROR");
    assert_eq!(c(s, Casing::Kebab), "http-error");
    assert_eq!(c(s, Casing::ScreamingKebab), "HTTP-ERROR");
}

#[test]
fn single_word() {
    assert_eq!(c("name", Casing::Pascal), "Name");
    assert_eq!(c("name", Casing::Camel), "name");
    assert_eq!(c("Name", Casing::Snake), "name");
    assert_eq!(c("Name", Casing::ScreamingSnake), "NAME");
}

#[test]
fn three_words() {
    let s = "first_name_here";
    assert_eq!(c(s, Casing::Camel), "firstNameHere");
    assert_eq!(c(s, Casing::Pascal), "FirstNameHere");
    assert_eq!(c(s, Casing::Kebab), "first-name-here");
    assert_eq!(c(s, Casing::ScreamingSnake), "FIRST_NAME_HERE");
}

#[test]
fn digits_stay_attached() {
    // A digit does not start a new word by itself, but a following
    // uppercase after a digit does (matches serde's boundary handling).
    assert_eq!(c("value2", Casing::Snake), "value2");
    assert_eq!(c("value2", Casing::Pascal), "Value2");
}

#[test]
fn const_context_and_pattern_position() {
    // The whole point: usable in const + match-pattern position.
    const K: &str = Casing::Camel.convert("user_id").as_str();
    assert_eq!(K, "userId");
    let got = matches!("userId", K);
    assert!(got);
}

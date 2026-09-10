//! `Option` fields spelled through a type alias (audit A3).
//!
//! A proc macro sees tokens, so a syntactic `Option<...>` test reads
//! `type MaybeName = Option<String>` as a required non-`Option` field.
//! That broke both directions: the writer emitted `"name":null` instead
//! of honouring `skip_if_none`, and the reader raised `MissingField`
//! for valid JSON that simply omitted the key — rejecting input that
//! any other producer would send.
//!
//! The shape is now decided by the type checker at the concrete call
//! site the derive expands to, so an alias behaves exactly like the
//! type it names. These cases pin both directions for direct `Option`,
//! plain aliases, generic aliases, and `Option<T>` with `T` a type
//! parameter, in named structs and enum variants.

use json_bourne::{FromJson, ToJson, parse_str, to_string};

type MaybeName = Option<String>;
type Maybe<T> = Option<T>;

#[derive(Debug, PartialEq, FromJson, ToJson)]
struct Aliased {
    id: u32,
    #[bourne(skip_if_none)]
    name: MaybeName,
}

#[derive(Debug, PartialEq, FromJson, ToJson)]
struct Direct {
    id: u32,
    #[bourne(skip_if_none)]
    name: Option<String>,
}

#[derive(Debug, PartialEq, FromJson, ToJson)]
struct GenericAlias {
    id: u32,
    #[bourne(skip_if_none)]
    count: Maybe<u8>,
}

/// No `skip_if_none`: the reader rule (absent key means `None`) has to
/// hold on its own.
#[derive(Debug, PartialEq, FromJson, ToJson)]
struct AliasedNoSkip {
    id: u32,
    name: MaybeName,
}

#[derive(Debug, PartialEq, FromJson, ToJson)]
struct GenericField<T> {
    id: u32,
    #[bourne(skip_if_none)]
    value: Option<T>,
}

/// `skip_if_none` on a field that is not an `Option` stays a documented
/// no-op rather than a compile error.
#[derive(Debug, PartialEq, FromJson, ToJson)]
struct Meaningless {
    #[bourne(skip_if_none)]
    id: u32,
}

#[derive(Debug, PartialEq, FromJson, ToJson)]
struct RenamedAlias {
    #[bourne(rename = "user-name", skip_if_none)]
    name: MaybeName,
    id: u32,
}

#[derive(Debug, PartialEq, FromJson, ToJson)]
enum Tagged {
    Rec {
        id: u32,
        #[bourne(skip_if_none)]
        note: MaybeName,
    },
}

#[derive(Debug, PartialEq, FromJson, ToJson)]
#[bourne(tag = "kind")]
enum Internal {
    Job {
        id: u32,
        #[bourne(skip_if_none)]
        note: MaybeName,
    },
}

#[test]
fn aliased_option_is_skipped_when_none() {
    let v = Aliased { id: 1, name: None };
    assert_eq!(to_string(&v).unwrap(), r#"{"id":1}"#);
}

#[test]
fn aliased_option_is_written_when_some() {
    let v = Aliased {
        id: 1,
        name: Some("x".into()),
    };
    assert_eq!(to_string(&v).unwrap(), r#"{"id":1,"name":"x"}"#);
}

#[test]
fn alias_and_direct_option_agree_on_output() {
    let aliased = Aliased { id: 7, name: None };
    let direct = Direct { id: 7, name: None };
    assert_eq!(to_string(&aliased).unwrap(), to_string(&direct).unwrap());
}

#[test]
fn generic_alias_is_skipped_when_none() {
    let v = GenericAlias { id: 2, count: None };
    assert_eq!(to_string(&v).unwrap(), r#"{"id":2}"#);
}

#[test]
fn generic_option_field_is_skipped_when_none() {
    let v: GenericField<u32> = GenericField { id: 3, value: None };
    assert_eq!(to_string(&v).unwrap(), r#"{"id":3}"#);
}

#[test]
fn option_without_skip_if_none_still_writes_null() {
    let v = AliasedNoSkip { id: 3, name: None };
    assert_eq!(to_string(&v).unwrap(), r#"{"id":3,"name":null}"#);
}

#[test]
fn skip_if_none_on_non_option_is_a_no_op() {
    let v = Meaningless { id: 4 };
    assert_eq!(to_string(&v).unwrap(), r#"{"id":4}"#);
}

#[test]
fn renamed_alias_skips_without_leaving_a_stray_comma() {
    let v = RenamedAlias { name: None, id: 9 };
    assert_eq!(to_string(&v).unwrap(), r#"{"id":9}"#);
}

#[test]
fn variant_alias_is_skipped_when_none() {
    let v = Tagged::Rec { id: 5, note: None };
    assert_eq!(to_string(&v).unwrap(), r#"{"Rec":{"id":5}}"#);
}

#[test]
fn internally_tagged_variant_alias_keeps_tag_comma() {
    let v = Internal::Job { id: 6, note: None };
    assert_eq!(to_string(&v).unwrap(), r#"{"kind":"Job","id":6}"#);

    let v = Internal::Job {
        id: 6,
        note: Some("n".into()),
    };
    assert_eq!(
        to_string(&v).unwrap(),
        r#"{"kind":"Job","id":6,"note":"n"}"#
    );
}

#[test]
fn aliased_option_reads_absent_key_as_none() {
    assert_eq!(
        parse_str::<Aliased>(r#"{"id":1}"#).unwrap(),
        Aliased { id: 1, name: None }
    );
}

#[test]
fn aliased_option_reads_explicit_null_as_none() {
    assert_eq!(
        parse_str::<Aliased>(r#"{"id":1,"name":null}"#).unwrap(),
        Aliased { id: 1, name: None }
    );
}

#[test]
fn aliased_option_reads_present_value() {
    assert_eq!(
        parse_str::<Aliased>(r#"{"id":1,"name":"v"}"#).unwrap(),
        Aliased {
            id: 1,
            name: Some("v".into())
        }
    );
}

#[test]
fn absent_key_is_none_without_skip_if_none() {
    assert_eq!(
        parse_str::<AliasedNoSkip>(r#"{"id":3}"#).unwrap(),
        AliasedNoSkip { id: 3, name: None }
    );
}

#[test]
fn generic_alias_reads_absent_key_as_none() {
    assert_eq!(
        parse_str::<GenericAlias>(r#"{"id":2}"#).unwrap(),
        GenericAlias { id: 2, count: None }
    );
}

#[test]
fn generic_option_field_reads_absent_key_as_none() {
    assert_eq!(
        parse_str::<GenericField<u32>>(r#"{"id":3}"#).unwrap(),
        GenericField { id: 3, value: None }
    );
}

#[test]
fn variant_alias_reads_absent_key_as_none() {
    assert_eq!(
        parse_str::<Tagged>(r#"{"Rec":{"id":5}}"#).unwrap(),
        Tagged::Rec { id: 5, note: None }
    );
}

#[test]
fn internally_tagged_variant_alias_reads_absent_key_as_none() {
    assert_eq!(
        parse_str::<Internal>(r#"{"kind":"Job","id":6}"#).unwrap(),
        Internal::Job { id: 6, note: None }
    );
}

#[test]
fn required_non_option_field_is_still_missing_when_absent() {
    let err = parse_str::<Aliased>(r#"{"name":"v"}"#).unwrap_err();
    assert_eq!(err.kind, json_bourne::ErrorKind::MissingField);
}

#[test]
fn aliased_option_round_trips_both_states() {
    for value in [
        Aliased { id: 1, name: None },
        Aliased {
            id: 2,
            name: Some("v".into()),
        },
    ] {
        let json = to_string(&value).unwrap();
        assert_eq!(parse_str::<Aliased>(&json).unwrap(), value, "json: {json}");
    }
}

#[test]
fn variant_alias_round_trips_both_states() {
    for value in [
        Tagged::Rec { id: 5, note: None },
        Tagged::Rec {
            id: 5,
            note: Some("n".into()),
        },
    ] {
        let json = to_string(&value).unwrap();
        assert_eq!(parse_str::<Tagged>(&json).unwrap(), value, "json: {json}");
    }
}

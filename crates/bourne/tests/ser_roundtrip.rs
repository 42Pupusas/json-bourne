//! Round-trip tests for [`ToJson`] primitives. Pairs each impl against
//! the existing `FromJson` impl: serialize a value, parse it back,
//! assert equality. This is the contract that the two sides agree on the
//! wire format — if a primitive's encoding ever drifts, one of these
//! tests breaks before any user code does.
//!
//! These exercise only the public API, so they live here rather than in
//! `src/tests/` and additionally pin that the surface is reachable from
//! outside the crate.

#![cfg(feature = "std")]

use json_bourne::{ErrorKind, parse_str, to_string};

/// Test helper: takes the value by value so call sites can pass
/// `rt(0_i64)` instead of `rt(&0_i64)`. The lint warning that this
/// "could take &T" is correct in the abstract but ergonomic loss
/// outweighs the (zero-cost) copy for `Copy` primitives, and the
/// owned `String` test variants are small.
#[allow(clippy::needless_pass_by_value)]
fn rt<T>(v: T)
where
    T: json_bourne::ToJson + for<'a> json_bourne::FromJson<'a> + PartialEq + core::fmt::Debug,
{
    let s = to_string(&v).expect("serialize");
    let v2: T = parse_str(&s).expect("parse back");
    assert_eq!(v, v2, "round-trip diverged via {s:?}");
}

#[test]
fn bool_roundtrips() {
    rt(true);
    rt(false);
}

#[test]
fn unit_roundtrips() {
    rt(());
}

#[test]
fn signed_ints_roundtrip() {
    rt(0_i8);
    rt(i8::MIN);
    rt(i8::MAX);
    rt(0_i16);
    rt(i16::MIN);
    rt(i16::MAX);
    rt(0_i32);
    rt(i32::MIN);
    rt(i32::MAX);
    rt(0_i64);
    rt(i64::MIN);
    rt(i64::MAX);
    rt(0_isize);
    rt(isize::MIN);
    rt(isize::MAX);
}

#[test]
fn unsigned_ints_roundtrip() {
    rt(0_u8);
    rt(u8::MAX);
    rt(0_u16);
    rt(u16::MAX);
    rt(0_u32);
    rt(u32::MAX);
    rt(0_u64);
    rt(u64::MAX);
    rt(0_usize);
    rt(usize::MAX);
}

#[test]
fn wide_ints_roundtrip() {
    rt(0_i128);
    rt(i128::MIN);
    rt(i128::MAX);
    rt(0_u128);
    rt(u128::MAX);
}

#[test]
fn strings_roundtrip() {
    // Plain ASCII, escapes, control bytes, non-ASCII UTF-8.
    for s in [
        "",
        "hello",
        "a\\b",
        "with \"quotes\"",
        "tab\tnewline\nreturn\rbs\x08ff\x0cnul\x00ctl\x1f",
        "café 中文 😀",
    ] {
        rt(String::from(s));
    }
}

/// Cow can't go through the generic `rt` helper — `FromJson<'a>` for
/// `Cow<'a, str>` ties the output lifetime to the input buffer, which
/// the HRTB the helper requires can't satisfy. Test the wire shape
/// inline instead.
#[test]
fn cow_roundtrips() {
    use std::borrow::Cow;
    for src in ["owned", "with\nescape", ""] {
        let v = Cow::<str>::Owned(String::from(src));
        let s = to_string(&v).unwrap();
        let back: Cow<'_, str> = parse_str(&s).unwrap();
        assert_eq!(back, src);
    }
}

#[test]
fn char_roundtrips() {
    for c in ['a', 'Z', '中', '😀', '\n', '\t', '"', '\\'] {
        rt(c);
    }
}

#[test]
fn option_roundtrips() {
    rt(Option::<u32>::None);
    rt(Some(42_u32));
    rt(Option::<String>::None);
    rt(Some(String::from("x")));
}

/// Reference impls forward through the inner value — verify the
/// blanket `&T` impl produces the same bytes as the owned form.
#[test]
fn reference_forwarding() {
    let v: u32 = 7;
    let owned = to_string(&v).unwrap();
    let by_ref = to_string(&&v).unwrap();
    assert_eq!(owned, by_ref);
}

/// Wrapper types are transparent — round-trip through the wrapper
/// must produce the same bytes as the inner value.
#[test]
fn wrapper_types_transparent() {
    use std::rc::Rc;
    use std::sync::Arc;
    let v: u32 = 9;
    assert_eq!(to_string(&v).unwrap(), to_string(&Box::new(v)).unwrap());
    assert_eq!(to_string(&v).unwrap(), to_string(&Rc::new(v)).unwrap());
    assert_eq!(to_string(&v).unwrap(), to_string(&Arc::new(v)).unwrap());
}

/// Pin the wire shape of a few primitives so any accidental
/// formatter change (digit grouping, capitalization, exponent
/// notation) shows up as a diff here, not as a downstream failure.
#[test]
fn pinned_wire_format() {
    assert_eq!(to_string(&true).unwrap(), "true");
    assert_eq!(to_string(&false).unwrap(), "false");
    assert_eq!(to_string(&()).unwrap(), "null");
    assert_eq!(to_string(&0_i64).unwrap(), "0");
    assert_eq!(to_string(&-1_i64).unwrap(), "-1");
    assert_eq!(to_string(&i64::MIN).unwrap(), "-9223372036854775808");
    assert_eq!(to_string(&u64::MAX).unwrap(), "18446744073709551615");
    assert_eq!(to_string(&"hi").unwrap(), "\"hi\"");
    // Control char inside a string → \u00XX.
    assert_eq!(to_string(&"\x01").unwrap(), "\"\\u0001\"");
}

#[test]
fn vec_roundtrips() {
    rt(Vec::<i32>::new());
    rt(vec![1_i32, 2, 3]);
    rt(vec![String::from("a"), String::from("b")]);
    rt(vec![Some(1_u32), None, Some(3)]);
}

#[test]
fn nested_vec_roundtrips() {
    rt(vec![vec![1_i32, 2], vec![3], Vec::new()]);
}

#[test]
fn fixed_array_roundtrips() {
    rt([1_i32, 2, 3]);
    rt([true, false, true]);
    let zero: [i32; 0] = [];
    rt(zero);
}

#[test]
fn tuple_roundtrips() {
    rt((1_i32, String::from("hi"), true));
    rt((1_u8, 2_u16, 3_u32, 4_u64));
}

#[test]
fn slice_pinned() {
    // Slice has no FromJson impl (you parse into Vec), so it can't
    // round-trip — pin its wire shape directly instead.
    let s: &[i32] = &[10, 20, 30];
    assert_eq!(to_string(&s).unwrap(), "[10,20,30]");
    let empty: &[u8] = &[];
    assert_eq!(to_string(&empty).unwrap(), "[]");
}

#[test]
fn btreemap_roundtrips() {
    use std::collections::BTreeMap;
    let mut m = BTreeMap::new();
    m.insert(String::from("a"), 1_i32);
    m.insert(String::from("b"), 2);
    rt(m);
}

#[test]
fn btreemap_with_escape_in_key() {
    use std::collections::BTreeMap;
    let mut m = BTreeMap::new();
    m.insert(String::from("a\nb"), 1_i32);
    rt(m);
}

#[test]
fn btreeset_roundtrips() {
    use std::collections::BTreeSet;
    let mut s = BTreeSet::new();
    s.insert(1_i32);
    s.insert(2);
    s.insert(3);
    rt(s);
}

#[test]
fn hashmap_roundtrips() {
    // HashMap iteration order isn't stable, so don't rt() — instead
    // serialize, parse back into HashMap, and compare maps directly.
    use std::collections::HashMap;
    let mut m = HashMap::new();
    m.insert(String::from("alpha"), 1_i32);
    m.insert(String::from("beta"), 2);
    let s = to_string(&m).unwrap();
    let back: HashMap<String, i32> = parse_str(&s).unwrap();
    assert_eq!(back, m);
}

#[test]
fn hashset_roundtrips() {
    use std::collections::HashSet;
    let mut s = HashSet::new();
    s.insert(1_i32);
    s.insert(2);
    s.insert(3);
    let json = to_string(&s).unwrap();
    let back: HashSet<i32> = parse_str(&json).unwrap();
    assert_eq!(back, s);
}

#[test]
fn ip_addrs_roundtrip() {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
    rt(Ipv4Addr::LOCALHOST);
    rt(Ipv6Addr::LOCALHOST);
    rt(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)));
    rt(SocketAddr::from(([127, 0, 0, 1], 8080)));
}

#[test]
fn pathbuf_roundtrips() {
    use std::path::PathBuf;
    rt(PathBuf::from("/etc/hosts"));
}

#[test]
fn empty_collections_pinned() {
    use std::collections::BTreeMap;
    assert_eq!(to_string(&Vec::<i32>::new()).unwrap(), "[]");
    let empty: [i32; 0] = [];
    assert_eq!(to_string(&empty).unwrap(), "[]");
    let m: BTreeMap<String, i32> = BTreeMap::new();
    assert_eq!(to_string(&m).unwrap(), "{}");
}

/// Floats via the production path (`write!`-based today, ryu later).
/// `rt()` works because `f64` parses back losslessly when serialized
/// via shortest-round-trip — that's the contract the `write!` impl
/// inherits from libstd's `Display` (which uses ryu internally).
#[test]
fn finite_f64_roundtrips() {
    for v in [
        0.0_f64,
        -0.0,
        1.0,
        -1.0,
        1.5,
        -1.5,
        1.5e2,
        1.5e-2,
        f64::MIN_POSITIVE,
        f64::MAX,
        f64::MIN,
        std::f64::consts::PI,
    ] {
        rt(v);
    }
}

#[test]
fn finite_f32_roundtrips() {
    for v in [0.0_f32, 1.5, -1.5, f32::MIN_POSITIVE, f32::MAX] {
        rt(v);
    }
}

#[test]
fn nonfinite_f64_rejected() {
    let r = to_string(&f64::INFINITY);
    assert_eq!(r.unwrap_err().kind, ErrorKind::NonFiniteFloat);
    let r = to_string(&f64::NEG_INFINITY);
    assert_eq!(r.unwrap_err().kind, ErrorKind::NonFiniteFloat);
    let r = to_string(&f64::NAN);
    assert_eq!(r.unwrap_err().kind, ErrorKind::NonFiniteFloat);
}

#[test]
fn duration_roundtrips() {
    use std::time::Duration;
    rt(Duration::from_secs(0));
    rt(Duration::from_millis(1500));
    rt(Duration::new(42, 750_000_000));
}

//! `from_json!` — declarative macro that emits a struct or enum together
//! with its [`FromJson`] impl.
//!
//! This is the zero-dep alternative to a `#[derive(FromJson)]` proc
//! macro. The crate's core promise is no external deps; a proc-macro
//! crate would need `proc-macro2` / `quote` / `syn`. `macro_rules!`
//! lives inside `bourne` itself, so the dep graph stays empty.
//!
//! # Usage
//!
//! ```
//! use bourne::{from_json, parse_str};
//!
//! from_json! {
//!     #[derive(Debug, PartialEq)]
//!     struct User<'input> {
//!         id: u64,
//!         name: &'input str,
//!         active: bool,
//!         nickname: Option<&'input str>,
//!     }
//! }
//!
//! let u: User<'_> = parse_str(r#"{"id":1,"name":"a","active":true}"#).unwrap();
//! assert_eq!(u.nickname, None);
//! ```
//!
//! # Strategy notes for maintainers
//!
//! `macro_rules!` has two traps that shape the design here:
//!
//! 1. **`:ty` capture is opaque downstream.** Once a type is captured
//!    via a `:ty` fragment specifier, splicing it into another macro's
//!    `tt`-pattern does not re-tokenize it — it remains a single
//!    opaque "type token" that downstream literal patterns like
//!    `Option<...>` cannot peek inside. So Option-vs-required
//!    detection must happen *during the same walk* where the type's
//!    individual tokens are still visible.
//!
//! 2. **Greedy `$($t:tt)+` swallows commas.** A naive
//!    `$fname:ident : $($fty:tt)+ , $($rest:tt)*` pattern doesn't
//!    work because `$($fty:tt)+` greedily eats commas it could match
//!    against `,`. The reliable workaround is a *tt-muncher*: walk
//!    one token at a time, accumulate into a "current type"
//!    accumulator until a `,` literal is hit, then commit and recurse.
//!
//! The body-walking macro below is one tt-muncher per field that
//! emits all three things (slot decl, key dispatch arm, final
//! assignment) at once, so each field is touched only once and the
//! type tokens are pattern-matchable for Option detection at every
//! step.

/// Emit a struct or enum together with its [`FromJson`] impl.
///
/// See the module-level docs for the supported shapes and limitations.
#[macro_export]
macro_rules! from_json {
    // -----------------------------------------------------------------
    // Lenient named-field struct (deny_unknown_fields = false), no generics.
    //
    // The container attribute must appear *first* among the outer
    // attrs. macro_rules! cannot pattern-match attrs after the fact
    // (each arm is a fixed prefix), so the literal placement matters.
    // -----------------------------------------------------------------
    (
        #[bourne(deny_unknown_fields = false)]
        $(#[$attr:meta])*
        $vis:vis struct $name:ident { $($body:tt)* }
    ) => {
        $crate::__from_json_emit_struct_def!(
            attrs: { $(#[$attr])* },
            vis: $vis,
            name: $name,
            generics_def: (),
            body: ($($body)*)
        );

        impl<'input> $crate::FromJson<'input> for $name {
            fn from_lex(__lex: &mut $crate::Lexer<'input>) -> ::core::result::Result<Self, $crate::Error> {
                $crate::__from_json_named_body!(__lex, (Self), lenient, $($body)*)
            }
        }
    };

    // Lenient + one lifetime.
    (
        #[bourne(deny_unknown_fields = false)]
        $(#[$attr:meta])*
        $vis:vis struct $name:ident < $lt:lifetime $(,)? > { $($body:tt)* }
    ) => {
        $crate::__from_json_emit_struct_def!(
            attrs: { $(#[$attr])* },
            vis: $vis,
            name: $name,
            generics_def: (<$lt>),
            body: ($($body)*)
        );

        impl<$lt> $crate::FromJson<$lt> for $name<$lt> {
            fn from_lex(__lex: &mut $crate::Lexer<$lt>) -> ::core::result::Result<Self, $crate::Error> {
                $crate::__from_json_named_body!(__lex, (Self), lenient, $($body)*)
            }
        }
    };

    // -----------------------------------------------------------------
    // Named-field struct, no generics.
    // -----------------------------------------------------------------
    (
        $(#[$attr:meta])*
        $vis:vis struct $name:ident { $($body:tt)* }
    ) => {
        $crate::__from_json_emit_struct_def!(
            attrs: { $(#[$attr])* },
            vis: $vis,
            name: $name,
            generics_def: (),
            body: ($($body)*)
        );

        impl<'input> $crate::FromJson<'input> for $name {
            fn from_lex(__lex: &mut $crate::Lexer<'input>) -> ::core::result::Result<Self, $crate::Error> {
                $crate::__from_json_named_body!(__lex, (Self), strict, $($body)*)
            }
        }
    };

    // -----------------------------------------------------------------
    // Named-field struct with one lifetime.
    // -----------------------------------------------------------------
    (
        $(#[$attr:meta])*
        $vis:vis struct $name:ident < $lt:lifetime $(,)? > { $($body:tt)* }
    ) => {
        $crate::__from_json_emit_struct_def!(
            attrs: { $(#[$attr])* },
            vis: $vis,
            name: $name,
            generics_def: (<$lt>),
            body: ($($body)*)
        );

        impl<$lt> $crate::FromJson<$lt> for $name<$lt> {
            fn from_lex(__lex: &mut $crate::Lexer<$lt>) -> ::core::result::Result<Self, $crate::Error> {
                $crate::__from_json_named_body!(__lex, (Self), strict, $($body)*)
            }
        }
    };

    // -----------------------------------------------------------------
    // Named-field struct with lifetime + one type param (bounded or not).
    // -----------------------------------------------------------------
    (
        $(#[$attr:meta])*
        $vis:vis struct $name:ident < $lt:lifetime, $tp:ident $(: $($tb:tt)+)? $(,)? > { $($body:tt)* }
    ) => {
        $crate::__from_json_emit_struct_def!(
            attrs: { $(#[$attr])* },
            vis: $vis,
            name: $name,
            generics_def: (<$lt, $tp $(: $($tb)+)?>),
            body: ($($body)*)
        );

        impl<$lt, $tp> $crate::FromJson<$lt> for $name<$lt, $tp>
        where
            $tp: $crate::FromJson<$lt> $($(+ $tb)+)?,
        {
            fn from_lex(__lex: &mut $crate::Lexer<$lt>) -> ::core::result::Result<Self, $crate::Error> {
                $crate::__from_json_named_body!(__lex, (Self), strict, $($body)*)
            }
        }
    };

    // -----------------------------------------------------------------
    // Named-field struct with one type param (no lifetime).
    // -----------------------------------------------------------------
    (
        $(#[$attr:meta])*
        $vis:vis struct $name:ident < $tp:ident $(: $($tb:tt)+)? $(,)? > { $($body:tt)* }
    ) => {
        $crate::__from_json_emit_struct_def!(
            attrs: { $(#[$attr])* },
            vis: $vis,
            name: $name,
            generics_def: (<$tp $(: $($tb)+)?>),
            body: ($($body)*)
        );

        impl<'input, $tp> $crate::FromJson<'input> for $name<$tp>
        where
            $tp: $crate::FromJson<'input> $($(+ $tb)+)?,
        {
            fn from_lex(__lex: &mut $crate::Lexer<'input>) -> ::core::result::Result<Self, $crate::Error> {
                $crate::__from_json_named_body!(__lex, (Self), strict, $($body)*)
            }
        }
    };

    // -----------------------------------------------------------------
    // Tuple struct, no generics, newtype (single field).
    //
    // Parses a *bare* JSON value of the inner type (matches serde's
    // `#[serde(transparent)]` default for newtypes).
    // -----------------------------------------------------------------
    (
        $(#[$attr:meta])*
        $vis:vis struct $name:ident ( $fty:ty $(,)? ) ;
    ) => {
        $(#[$attr])*
        $vis struct $name($fty);

        impl<'input> $crate::FromJson<'input> for $name {
            fn from_lex(__lex: &mut $crate::Lexer<'input>) -> ::core::result::Result<Self, $crate::Error> {
                ::core::result::Result::Ok(Self(
                    <$fty as $crate::FromJson<'_>>::from_lex(__lex)?,
                ))
            }
        }
    };

    // -----------------------------------------------------------------
    // Tuple struct with one lifetime, newtype.
    //
    // The lifetime threads through into the inner type's borrow chain
    // (e.g. `BorrowedTag<'input>(&'input str)`).
    // -----------------------------------------------------------------
    (
        $(#[$attr:meta])*
        $vis:vis struct $name:ident < $lt:lifetime $(,)? > ( $fty:ty $(,)? ) ;
    ) => {
        $(#[$attr])*
        $vis struct $name<$lt>($fty);

        impl<$lt> $crate::FromJson<$lt> for $name<$lt> {
            fn from_lex(__lex: &mut $crate::Lexer<$lt>) -> ::core::result::Result<Self, $crate::Error> {
                ::core::result::Result::Ok(Self(
                    <$fty as $crate::FromJson<$lt>>::from_lex(__lex)?,
                ))
            }
        }
    };

    // -----------------------------------------------------------------
    // Tuple struct, multi-field. No generics.
    //
    // Parses a JSON array of exactly N elements, where N is the number
    // of declared fields. The element-walker emits per-element reads
    // separated by `array_continue` checks; the closing-bracket check
    // after the last element rejects "array too long" inputs.
    // -----------------------------------------------------------------
    (
        $(#[$attr:meta])*
        $vis:vis struct $name:ident ( $fty1:ty, $($ftyn:ty),+ $(,)? ) ;
    ) => {
        $(#[$attr])*
        $vis struct $name($fty1, $($ftyn),+);

        impl<'input> $crate::FromJson<'input> for $name {
            fn from_lex(__lex: &mut $crate::Lexer<'input>) -> ::core::result::Result<Self, $crate::Error> {
                if __lex.array_start()? {
                    return ::core::result::Result::Err(
                        $crate::Error::new($crate::ErrorKind::TypeMismatch, __lex.position()),
                    );
                }
                let __elem_0 = <$fty1 as $crate::FromJson<'_>>::from_lex(__lex)?;
                $crate::__from_json_tuple_walk!(
                    lex: __lex,
                    self_ctor: (Self),
                    accum: [ __elem_0 ],
                    remaining: [ $(($ftyn))+ ]
                )
            }
        }
    };

    // Tuple struct, multi-field, with one lifetime.
    (
        $(#[$attr:meta])*
        $vis:vis struct $name:ident < $lt:lifetime $(,)? > ( $fty1:ty, $($ftyn:ty),+ $(,)? ) ;
    ) => {
        $(#[$attr])*
        $vis struct $name<$lt>($fty1, $($ftyn),+);

        impl<$lt> $crate::FromJson<$lt> for $name<$lt> {
            fn from_lex(__lex: &mut $crate::Lexer<$lt>) -> ::core::result::Result<Self, $crate::Error> {
                if __lex.array_start()? {
                    return ::core::result::Result::Err(
                        $crate::Error::new($crate::ErrorKind::TypeMismatch, __lex.position()),
                    );
                }
                let __elem_0 = <$fty1 as $crate::FromJson<$lt>>::from_lex(__lex)?;
                $crate::__from_json_tuple_walk!(
                    lex: __lex,
                    self_ctor: (Self),
                    accum: [ __elem_0 ],
                    remaining: [ $(($ftyn))+ ]
                )
            }
        }
    };

    // -----------------------------------------------------------------
    // Externally-tagged enum, no generics.
    //
    // JSON shape per variant:
    //   - Unit `Foo`             → string `"Foo"`
    //   - Newtype `Foo(T)`       → object `{"Foo": <T>}`
    //   - Tuple `Foo(T, U)`      → object `{"Foo": [<T>, <U>]}`
    //   - Struct `Foo {a, b}`    → object `{"Foo": {"a": …, "b": …}}`
    //
    // Variant-level `#[bourne(rename = "...")]` retags the variant.
    // -----------------------------------------------------------------
    (
        $(#[$attr:meta])*
        $vis:vis enum $name:ident { $($variants:tt)* }
    ) => {
        $crate::__from_json_emit_enum_def!(
            attrs: { $(#[$attr])* },
            vis: $vis,
            name: $name,
            generics_def: (),
            variants_input: ($($variants)*)
        );

        impl<'input> $crate::FromJson<'input> for $name {
            fn from_lex(__lex: &mut $crate::Lexer<'input>) -> ::core::result::Result<Self, $crate::Error> {
                $crate::__from_json_enum_dispatch!(__lex, $name, ($($variants)*))
            }
        }
    };

    // Externally-tagged enum, one lifetime.
    (
        $(#[$attr:meta])*
        $vis:vis enum $name:ident < $lt:lifetime $(,)? > { $($variants:tt)* }
    ) => {
        $crate::__from_json_emit_enum_def!(
            attrs: { $(#[$attr])* },
            vis: $vis,
            name: $name,
            generics_def: (<$lt>),
            variants_input: ($($variants)*)
        );

        impl<$lt> $crate::FromJson<$lt> for $name<$lt> {
            fn from_lex(__lex: &mut $crate::Lexer<$lt>) -> ::core::result::Result<Self, $crate::Error> {
                $crate::__from_json_enum_dispatch!(__lex, $name, ($($variants)*))
            }
        }
    };
}

// ============================================================================
// Struct-definition emitter (with field-attribute strip).
//
// Walks the body tt-stream and produces a clean struct definition
// where `#[bourne(...)]` attrs have been removed but every other attr
// (including `#[doc = "..."]`, user-written `#[allow(...)]`, etc.) is
// preserved on the field. The impl walker downstream re-walks the
// same body to extract the bourne-attr metadata for slot decls,
// dispatch arms, and final assignments.
//
// Two walks over the same body is wasteful in macro-expansion terms
// but simpler than threading both outputs through one combined
// walker. The body is bounded by the user's struct definition, so
// the constant factor is small.
//
// State:
//   - kept: the stripped body emitted so far
//   - cur:  per-field accumulator for "attrs we'll keep on this field"
//   - input: remaining tokens
// ============================================================================

#[doc(hidden)]
#[macro_export]
macro_rules! __from_json_emit_struct_def {
    (
        attrs: { $(#[$attr:meta])* },
        vis: $vis:vis,
        name: $name:ident,
        generics_def: (),
        body: ($($body:tt)*)
    ) => {
        $crate::__from_json_strip_walk!(
            done: {
                attrs: { $(#[$attr])* },
                vis: $vis,
                name: $name,
                generics: ()
            },
            kept: { },
            cur: [],
            input: ($($body)*)
        );
    };
    (
        attrs: { $(#[$attr:meta])* },
        vis: $vis:vis,
        name: $name:ident,
        generics_def: (<$lt:lifetime>),
        body: ($($body:tt)*)
    ) => {
        $crate::__from_json_strip_walk!(
            done: {
                attrs: { $(#[$attr])* },
                vis: $vis,
                name: $name,
                generics: (<$lt>)
            },
            kept: { },
            cur: [],
            input: ($($body)*)
        );
    };
    (
        attrs: { $(#[$attr:meta])* },
        vis: $vis:vis,
        name: $name:ident,
        generics_def: (<$tp:ident $(: $($tb:tt)+)?>),
        body: ($($body:tt)*)
    ) => {
        $crate::__from_json_strip_walk!(
            done: {
                attrs: { $(#[$attr])* },
                vis: $vis,
                name: $name,
                generics: (<$tp $(: $($tb)+)?>)
            },
            kept: { },
            cur: [],
            input: ($($body)*)
        );
    };
    (
        attrs: { $(#[$attr:meta])* },
        vis: $vis:vis,
        name: $name:ident,
        generics_def: (<$lt:lifetime, $tp:ident $(: $($tb:tt)+)?>),
        body: ($($body:tt)*)
    ) => {
        $crate::__from_json_strip_walk!(
            done: {
                attrs: { $(#[$attr])* },
                vis: $vis,
                name: $name,
                generics: (<$lt, $tp $(: $($tb)+)?>)
            },
            kept: { },
            cur: [],
            input: ($($body)*)
        );
    };
}

// Strip-pass walker. Eats tokens from `input`. When it sees a
// `#[bourne(...)]` group, drops it. When it sees any other `#[...]`
// group, appends it to `cur` (per-field). When it reaches the field
// name + colon + type tokens up to a comma, commits `cur + field` to
// `kept` and starts the next field.
//
// At end-of-input, splices `kept` into the struct definition.
#[doc(hidden)]
#[macro_export]
macro_rules! __from_json_strip_walk {
    // Done: emit the struct.
    (
        done: {
            attrs: { $(#[$outer:meta])* },
            vis: $vis:vis,
            name: $name:ident,
            generics: ()
        },
        kept: { $($kept:tt)* },
        cur: [],
        input: ()
    ) => {
        $(#[$outer])*
        $vis struct $name { $($kept)* }
    };
    (
        done: {
            attrs: { $(#[$outer:meta])* },
            vis: $vis:vis,
            name: $name:ident,
            generics: ($($g:tt)+)
        },
        kept: { $($kept:tt)* },
        cur: [],
        input: ()
    ) => {
        $(#[$outer])*
        $vis struct $name $($g)+ { $($kept)* }
    };

    // bourne(rename = "x") attribute: drop it.
    (
        done: $done:tt,
        kept: { $($kept:tt)* },
        cur: [ $($cur:tt)* ],
        input: ( #[bourne(rename = $_lit:literal)] $($rest:tt)* )
    ) => {
        $crate::__from_json_strip_walk!(
            done: $done,
            kept: { $($kept)* },
            cur: [ $($cur)* ],
            input: ($($rest)*)
        );
    };
    // bourne(default) attribute: drop it.
    (
        done: $done:tt,
        kept: { $($kept:tt)* },
        cur: [ $($cur:tt)* ],
        input: ( #[bourne(default)] $($rest:tt)* )
    ) => {
        $crate::__from_json_strip_walk!(
            done: $done,
            kept: { $($kept)* },
            cur: [ $($cur)* ],
            input: ($($rest)*)
        );
    };
    // bourne(default = "fn") attribute: drop it.
    (
        done: $done:tt,
        kept: { $($kept:tt)* },
        cur: [ $($cur:tt)* ],
        input: ( #[bourne(default = $_path:literal)] $($rest:tt)* )
    ) => {
        $crate::__from_json_strip_walk!(
            done: $done,
            kept: { $($kept)* },
            cur: [ $($cur)* ],
            input: ($($rest)*)
        );
    };
    // bourne(skip) attribute: drop it.
    (
        done: $done:tt,
        kept: { $($kept:tt)* },
        cur: [ $($cur:tt)* ],
        input: ( #[bourne(skip)] $($rest:tt)* )
    ) => {
        $crate::__from_json_strip_walk!(
            done: $done,
            kept: { $($kept)* },
            cur: [ $($cur)* ],
            input: ($($rest)*)
        );
    };
    // bourne(rename = "x", default) compound: drop it.
    (
        done: $done:tt,
        kept: { $($kept:tt)* },
        cur: [ $($cur:tt)* ],
        input: ( #[bourne(rename = $_lit:literal, default)] $($rest:tt)* )
    ) => {
        $crate::__from_json_strip_walk!(
            done: $done,
            kept: { $($kept)* },
            cur: [ $($cur)* ],
            input: ($($rest)*)
        );
    };
    // Any other `#[...]` attribute: keep it on `cur`.
    (
        done: $done:tt,
        kept: { $($kept:tt)* },
        cur: [ $($cur:tt)* ],
        input: ( #[$other:meta] $($rest:tt)* )
    ) => {
        $crate::__from_json_strip_walk!(
            done: $done,
            kept: { $($kept)* },
            cur: [ $($cur)* #[$other] ],
            input: ($($rest)*)
        );
    };

    // Field with vis + name + colon + type ending at `,`.
    // Use `:ty` here (the *struct definition* is a context where
    // collapsed type tokens are fine — we re-emit them verbatim).
    (
        done: $done:tt,
        kept: { $($kept:tt)* },
        cur: [ $($cur:tt)* ],
        input: ( $fvis:vis $fname:ident : $fty:ty , $($rest:tt)* )
    ) => {
        $crate::__from_json_strip_walk!(
            done: $done,
            kept: {
                $($kept)*
                $($cur)* $fvis $fname: $fty,
            },
            cur: [],
            input: ($($rest)*)
        );
    };
    // Last field, no trailing comma.
    (
        done: $done:tt,
        kept: { $($kept:tt)* },
        cur: [ $($cur:tt)* ],
        input: ( $fvis:vis $fname:ident : $fty:ty )
    ) => {
        $crate::__from_json_strip_walk!(
            done: $done,
            kept: {
                $($kept)*
                $($cur)* $fvis $fname: $fty,
            },
            cur: [],
            input: ()
        );
    };
}

// ============================================================================
// Enum-definition emitter (with variant-attribute strip).
//
// Walks the variant list and produces a clean enum definition where
// `#[bourne(rename = "...")]` attrs have been removed but every other
// attr is preserved. Same two-walk approach as the named-struct
// strip pass.
// ============================================================================

#[doc(hidden)]
#[macro_export]
macro_rules! __from_json_emit_enum_def {
    (
        attrs: { $(#[$attr:meta])* },
        vis: $vis:vis,
        name: $name:ident,
        generics_def: (),
        variants_input: ($($variants:tt)*)
    ) => {
        $crate::__from_json_enum_strip!(
            done: {
                attrs: { $(#[$attr])* },
                vis: $vis,
                name: $name,
                generics: ()
            },
            kept: { },
            cur: [],
            input: ($($variants)*)
        );
    };
    (
        attrs: { $(#[$attr:meta])* },
        vis: $vis:vis,
        name: $name:ident,
        generics_def: (<$lt:lifetime>),
        variants_input: ($($variants:tt)*)
    ) => {
        $crate::__from_json_enum_strip!(
            done: {
                attrs: { $(#[$attr])* },
                vis: $vis,
                name: $name,
                generics: (<$lt>)
            },
            kept: { },
            cur: [],
            input: ($($variants)*)
        );
    };
}

// Strip-pass walker for enums. State:
//   - kept: stripped variants emitted so far
//   - cur: per-variant accumulator for kept attrs
//   - input: remaining tokens
//
// Variant body recognition:
//   - `(types)` (paren group) — tuple/newtype variant
//   - `{fields}` (brace group) — struct variant
//   - neither — unit variant
//
// macro_rules! groups parens and braces as single tt items, so we
// can match the body shape via separate arms.
#[doc(hidden)]
#[macro_export]
macro_rules! __from_json_enum_strip {
    // Done.
    (
        done: {
            attrs: { $(#[$outer:meta])* },
            vis: $vis:vis,
            name: $name:ident,
            generics: ()
        },
        kept: { $($kept:tt)* },
        cur: [],
        input: ()
    ) => {
        $(#[$outer])*
        $vis enum $name { $($kept)* }
    };
    (
        done: {
            attrs: { $(#[$outer:meta])* },
            vis: $vis:vis,
            name: $name:ident,
            generics: ($($g:tt)+)
        },
        kept: { $($kept:tt)* },
        cur: [],
        input: ()
    ) => {
        $(#[$outer])*
        $vis enum $name $($g)+ { $($kept)* }
    };

    // bourne(rename = "x") on the next variant: drop it.
    (
        done: $done:tt,
        kept: { $($kept:tt)* },
        cur: [ $($cur:tt)* ],
        input: ( #[bourne(rename = $_lit:literal)] $($rest:tt)* )
    ) => {
        $crate::__from_json_enum_strip!(
            done: $done,
            kept: { $($kept)* },
            cur: [ $($cur)* ],
            input: ($($rest)*)
        );
    };
    // Other attribute: keep on cur.
    (
        done: $done:tt,
        kept: { $($kept:tt)* },
        cur: [ $($cur:tt)* ],
        input: ( #[$other:meta] $($rest:tt)* )
    ) => {
        $crate::__from_json_enum_strip!(
            done: $done,
            kept: { $($kept)* },
            cur: [ $($cur)* #[$other] ],
            input: ($($rest)*)
        );
    };

    // Unit variant followed by `,`.
    (
        done: $done:tt,
        kept: { $($kept:tt)* },
        cur: [ $($cur:tt)* ],
        input: ( $vname:ident , $($rest:tt)* )
    ) => {
        $crate::__from_json_enum_strip!(
            done: $done,
            kept: { $($kept)* $($cur)* $vname, },
            cur: [],
            input: ($($rest)*)
        );
    };
    // Unit variant, last (no trailing comma).
    (
        done: $done:tt,
        kept: { $($kept:tt)* },
        cur: [ $($cur:tt)* ],
        input: ( $vname:ident )
    ) => {
        $crate::__from_json_enum_strip!(
            done: $done,
            kept: { $($kept)* $($cur)* $vname, },
            cur: [],
            input: ()
        );
    };

    // Tuple/newtype variant (parenthesized body) with trailing `,`.
    (
        done: $done:tt,
        kept: { $($kept:tt)* },
        cur: [ $($cur:tt)* ],
        input: ( $vname:ident ( $($body:tt)* ) , $($rest:tt)* )
    ) => {
        $crate::__from_json_enum_strip!(
            done: $done,
            kept: { $($kept)* $($cur)* $vname($($body)*), },
            cur: [],
            input: ($($rest)*)
        );
    };
    // Tuple/newtype variant, last.
    (
        done: $done:tt,
        kept: { $($kept:tt)* },
        cur: [ $($cur:tt)* ],
        input: ( $vname:ident ( $($body:tt)* ) )
    ) => {
        $crate::__from_json_enum_strip!(
            done: $done,
            kept: { $($kept)* $($cur)* $vname($($body)*), },
            cur: [],
            input: ()
        );
    };

    // Struct variant (brace body) with trailing `,`.
    //
    // The body may contain `#[bourne(...)]` field attrs that must
    // also be stripped; route the body through the named-struct
    // strip walker by pre-emitting it here as opaque tokens, since
    // bourne attrs on inner fields are forbidden inside enum struct
    // variants for v1 (the proc-macro version's design supported
    // them but `macro_rules!` recursion with a separate output sink
    // for the body would balloon the macro size; v1 punts).
    (
        done: $done:tt,
        kept: { $($kept:tt)* },
        cur: [ $($cur:tt)* ],
        input: ( $vname:ident { $($body:tt)* } , $($rest:tt)* )
    ) => {
        $crate::__from_json_enum_strip!(
            done: $done,
            kept: { $($kept)* $($cur)* $vname { $($body)* }, },
            cur: [],
            input: ($($rest)*)
        );
    };
    // Struct variant, last.
    (
        done: $done:tt,
        kept: { $($kept:tt)* },
        cur: [ $($cur:tt)* ],
        input: ( $vname:ident { $($body:tt)* } )
    ) => {
        $crate::__from_json_enum_strip!(
            done: $done,
            kept: { $($kept)* $($cur)* $vname { $($body)* }, },
            cur: [],
            input: ()
        );
    };
}

// ============================================================================
// Enum dispatch (impl side).
//
// Walks the variant list once to build:
//   - unit_arms: match arms for the JSON-string ValueKind
//   - tagged_arms: match arms for the JSON-object ValueKind
//
// At end-of-input, splices both into a `match peek_value_kind` block
// with a `_ => TypeMismatch` catchall.
// ============================================================================

#[doc(hidden)]
#[macro_export]
macro_rules! __from_json_enum_dispatch {
    ($lex:ident, $name:ident, ($($variants:tt)*)) => {
        $crate::__from_json_enum_walk!(
            lex: $lex,
            name: $name,
            unit_arms: { },
            tagged_arms: { },
            cur_rename: (),
            has_tagged: (),
            input: ($($variants)*)
        )
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __from_json_enum_walk {
    // Done — all-unit enum: skip the Object dispatch entirely (it
    // would be empty and would generate `unreachable_code` warnings).
    (
        lex: $lex:ident,
        name: $name:ident,
        unit_arms: { $($unit:tt)* },
        tagged_arms: { },
        cur_rename: (),
        has_tagged: (),
        input: ()
    ) => {
        match $lex.peek_value_kind()? {
            $crate::ValueKind::String => {
                let __tag = $lex.parse_str_value()?;
                match __tag {
                    $($unit)*
                    _ => ::core::result::Result::Err(
                        $crate::Error::new($crate::ErrorKind::UnknownField, $lex.position()),
                    ),
                }
            }
            _ => ::core::result::Result::Err(
                $crate::Error::new($crate::ErrorKind::TypeMismatch, $lex.position()),
            ),
        }
    };

    // Done — has at least one tagged variant: full String + Object dispatch.
    (
        lex: $lex:ident,
        name: $name:ident,
        unit_arms: { $($unit:tt)* },
        tagged_arms: { $($tagged:tt)* },
        cur_rename: (),
        has_tagged: ($_h:tt $($_hrest:tt)*),
        input: ()
    ) => {
        match $lex.peek_value_kind()? {
            $crate::ValueKind::String => {
                let __tag = $lex.parse_str_value()?;
                match __tag {
                    $($unit)*
                    _ => ::core::result::Result::Err(
                        $crate::Error::new($crate::ErrorKind::UnknownField, $lex.position()),
                    ),
                }
            }
            $crate::ValueKind::Object => {
                $lex.object_start()?;
                let __key_js = $lex.object_first_key_lex()?.ok_or_else(|| {
                    $crate::Error::new($crate::ErrorKind::UnknownField, $lex.position())
                })?;
                let __key_cow = $crate::key_to_cow(__key_js, $lex)?;
                let __value = match __key_cow.as_ref() {
                    $($tagged)*
                    _ => return ::core::result::Result::Err(
                        $crate::Error::new($crate::ErrorKind::UnknownField, $lex.position()),
                    ),
                };
                if $lex.object_next_key_lex()?.is_some() {
                    return ::core::result::Result::Err(
                        $crate::Error::new($crate::ErrorKind::UnknownField, $lex.position()),
                    );
                }
                ::core::result::Result::Ok(__value)
            }
            _ => ::core::result::Result::Err(
                $crate::Error::new($crate::ErrorKind::TypeMismatch, $lex.position()),
            ),
        }
    };

    // bourne(rename = "x") on the next variant.
    (
        lex: $lex:ident,
        name: $name:ident,
        unit_arms: $u:tt,
        tagged_arms: $t:tt,
        cur_rename: (),
        has_tagged: ($($h:tt)*),
        input: ( #[bourne(rename = $renamed:literal)] $($rest:tt)* )
    ) => {
        $crate::__from_json_enum_walk!(
            lex: $lex,
            name: $name,
            unit_arms: $u,
            tagged_arms: $t,
            cur_rename: ($renamed),
            has_tagged: ($($h)*),
            input: ($($rest)*)
        )
    };

    // Skip other attributes on the variant.
    (
        lex: $lex:ident,
        name: $name:ident,
        unit_arms: $u:tt,
        tagged_arms: $t:tt,
        cur_rename: ($($rename:tt)?),
        has_tagged: ($($h:tt)*),
        input: ( #[$_other:meta] $($rest:tt)* )
    ) => {
        $crate::__from_json_enum_walk!(
            lex: $lex,
            name: $name,
            unit_arms: $u,
            tagged_arms: $t,
            cur_rename: ($($rename)?),
            has_tagged: ($($h)*),
            input: ($($rest)*)
        )
    };

    // Unit variant followed by `,`.
    (
        lex: $lex:ident,
        name: $name:ident,
        unit_arms: { $($u:tt)* },
        tagged_arms: $t:tt,
        cur_rename: ($($rename:tt)?),
        has_tagged: ($($h:tt)*),
        input: ( $vname:ident , $($rest:tt)* )
    ) => {
        $crate::__from_json_enum_walk!(
            lex: $lex,
            name: $name,
            unit_arms: {
                $($u)*
                $crate::__from_json_field_key!($vname, ($($rename)?))
                    => ::core::result::Result::Ok($name::$vname),
            },
            tagged_arms: $t,
            cur_rename: (),
            has_tagged: ($($h)*),
            input: ($($rest)*)
        )
    };
    // Unit variant, last.
    (
        lex: $lex:ident,
        name: $name:ident,
        unit_arms: { $($u:tt)* },
        tagged_arms: $t:tt,
        cur_rename: ($($rename:tt)?),
        has_tagged: ($($h:tt)*),
        input: ( $vname:ident )
    ) => {
        $crate::__from_json_enum_walk!(
            lex: $lex,
            name: $name,
            unit_arms: {
                $($u)*
                $crate::__from_json_field_key!($vname, ($($rename)?))
                    => ::core::result::Result::Ok($name::$vname),
            },
            tagged_arms: $t,
            cur_rename: (),
            has_tagged: ($($h)*),
            input: ()
        )
    };

    // Newtype variant (single-element tuple).
    (
        lex: $lex:ident,
        name: $name:ident,
        unit_arms: $u:tt,
        tagged_arms: { $($t:tt)* },
        cur_rename: ($($rename:tt)?),
        has_tagged: ($($h:tt)*),
        input: ( $vname:ident ( $fty:ty $(,)? ) , $($rest:tt)* )
    ) => {
        $crate::__from_json_enum_walk!(
            lex: $lex,
            name: $name,
            unit_arms: $u,
            tagged_arms: {
                $($t)*
                $crate::__from_json_field_key!($vname, ($($rename)?)) => $name::$vname(
                    <$fty as $crate::FromJson<'_>>::from_lex($lex)?,
                ),
            },
            cur_rename: (),
            has_tagged: ($($h)* ()),
            input: ($($rest)*)
        )
    };
    (
        lex: $lex:ident,
        name: $name:ident,
        unit_arms: $u:tt,
        tagged_arms: { $($t:tt)* },
        cur_rename: ($($rename:tt)?),
        has_tagged: ($($h:tt)*),
        input: ( $vname:ident ( $fty:ty $(,)? ) )
    ) => {
        $crate::__from_json_enum_walk!(
            lex: $lex,
            name: $name,
            unit_arms: $u,
            tagged_arms: {
                $($t)*
                $crate::__from_json_field_key!($vname, ($($rename)?)) => $name::$vname(
                    <$fty as $crate::FromJson<'_>>::from_lex($lex)?,
                ),
            },
            cur_rename: (),
            has_tagged: ($($h)* ()),
            input: ()
        )
    };

    // Multi-field tuple variant.
    (
        lex: $lex:ident,
        name: $name:ident,
        unit_arms: $u:tt,
        tagged_arms: { $($t:tt)* },
        cur_rename: ($($rename:tt)?),
        has_tagged: ($($h:tt)*),
        input: ( $vname:ident ( $fty1:ty, $($ftyn:ty),+ $(,)? ) , $($rest:tt)* )
    ) => {
        $crate::__from_json_enum_walk!(
            lex: $lex,
            name: $name,
            unit_arms: $u,
            tagged_arms: {
                $($t)*
                $crate::__from_json_field_key!($vname, ($($rename)?)) => {
                    if $lex.array_start()? {
                        return ::core::result::Result::Err(
                            $crate::Error::new($crate::ErrorKind::TypeMismatch, $lex.position()),
                        );
                    }
                    let __elem_0 = <$fty1 as $crate::FromJson<'_>>::from_lex($lex)?;
                    $crate::__from_json_tuple_walk!(
                        lex: $lex,
                        self_ctor: ($name::$vname),
                        accum: [ __elem_0 ],
                        remaining: [ $(($ftyn))+ ]
                    )?
                },
            },
            cur_rename: (),
            has_tagged: ($($h)* ()),
            input: ($($rest)*)
        )
    };
    (
        lex: $lex:ident,
        name: $name:ident,
        unit_arms: $u:tt,
        tagged_arms: { $($t:tt)* },
        cur_rename: ($($rename:tt)?),
        has_tagged: ($($h:tt)*),
        input: ( $vname:ident ( $fty1:ty, $($ftyn:ty),+ $(,)? ) )
    ) => {
        $crate::__from_json_enum_walk!(
            lex: $lex,
            name: $name,
            unit_arms: $u,
            tagged_arms: {
                $($t)*
                $crate::__from_json_field_key!($vname, ($($rename)?)) => {
                    if $lex.array_start()? {
                        return ::core::result::Result::Err(
                            $crate::Error::new($crate::ErrorKind::TypeMismatch, $lex.position()),
                        );
                    }
                    let __elem_0 = <$fty1 as $crate::FromJson<'_>>::from_lex($lex)?;
                    $crate::__from_json_tuple_walk!(
                        lex: $lex,
                        self_ctor: ($name::$vname),
                        accum: [ __elem_0 ],
                        remaining: [ $(($ftyn))+ ]
                    )?
                },
            },
            cur_rename: (),
            has_tagged: ($($h)* ()),
            input: ()
        )
    };

    // Struct variant.
    (
        lex: $lex:ident,
        name: $name:ident,
        unit_arms: $u:tt,
        tagged_arms: { $($t:tt)* },
        cur_rename: ($($rename:tt)?),
        has_tagged: ($($h:tt)*),
        input: ( $vname:ident { $($body:tt)* } , $($rest:tt)* )
    ) => {
        $crate::__from_json_enum_walk!(
            lex: $lex,
            name: $name,
            unit_arms: $u,
            tagged_arms: {
                $($t)*
                $crate::__from_json_field_key!($vname, ($($rename)?)) => {
                    $crate::__from_json_named_body!(
                        $lex,
                        ($name::$vname),
                        strict,
                        $($body)*
                    )?
                },
            },
            cur_rename: (),
            has_tagged: ($($h)* ()),
            input: ($($rest)*)
        )
    };
    (
        lex: $lex:ident,
        name: $name:ident,
        unit_arms: $u:tt,
        tagged_arms: { $($t:tt)* },
        cur_rename: ($($rename:tt)?),
        has_tagged: ($($h:tt)*),
        input: ( $vname:ident { $($body:tt)* } )
    ) => {
        $crate::__from_json_enum_walk!(
            lex: $lex,
            name: $name,
            unit_arms: $u,
            tagged_arms: {
                $($t)*
                $crate::__from_json_field_key!($vname, ($($rename)?)) => {
                    $crate::__from_json_named_body!(
                        $lex,
                        ($name::$vname),
                        strict,
                        $($body)*
                    )?
                },
            },
            cur_rename: (),
            has_tagged: ($($h)* ()),
            input: ()
        )
    };
}

// ============================================================================
// Tuple struct element walker.
//
// State: `accum` holds the names of elements read so far; `remaining`
// holds the types still to read. Each step consumes one type from
// `remaining`, emits a `array_continue == false` check, reads the
// element via FromJson, and recurses. When `remaining` is empty,
// emits the closing `array_continue == true` check and constructs.
//
// The element naming is positional: `__elem_0`, `__elem_1`, ... up to
// `__elem_7`. Tuple structs of more than 8 fields are vanishingly rare
// in real code; we cap at 8 because each new ident has to be hand-
// minted in this macro (no concat_idents! on stable). If users hit the
// cap, they can hand-write the FromJson impl.
// ============================================================================

#[doc(hidden)]
#[macro_export]
macro_rules! __from_json_tuple_walk {
    // Done — emit the closing check and the constructor.
    (
        lex: $lex:ident,
        self_ctor: ($($ctor:tt)+),
        accum: [ $($prev:ident),+ ],
        remaining: []
    ) => {{
        if !$lex.array_continue(b']')? {
            return ::core::result::Result::Err(
                $crate::Error::new($crate::ErrorKind::TypeMismatch, $lex.position()),
            );
        }
        ::core::result::Result::Ok($($ctor)+($($prev),+))
    }};

    // Read next element. Position-based naming via inner sub-arms.
    (lex: $lex:ident, self_ctor: ($($ctor:tt)+), accum: [ $a:ident ], remaining: [ ($fty:ty) $($rest:tt)* ]) => {{
        if $lex.array_continue(b']')? {
            return ::core::result::Result::Err(
                $crate::Error::new($crate::ErrorKind::TypeMismatch, $lex.position()),
            );
        }
        let __elem_1 = <$fty as $crate::FromJson<'_>>::from_lex($lex)?;
        $crate::__from_json_tuple_walk!(
            lex: $lex, self_ctor: ($($ctor)+), accum: [ $a, __elem_1 ], remaining: [ $($rest)* ]
        )
    }};
    (lex: $lex:ident, self_ctor: ($($ctor:tt)+), accum: [ $a:ident, $b:ident ], remaining: [ ($fty:ty) $($rest:tt)* ]) => {{
        if $lex.array_continue(b']')? {
            return ::core::result::Result::Err(
                $crate::Error::new($crate::ErrorKind::TypeMismatch, $lex.position()),
            );
        }
        let __elem_2 = <$fty as $crate::FromJson<'_>>::from_lex($lex)?;
        $crate::__from_json_tuple_walk!(
            lex: $lex, self_ctor: ($($ctor)+), accum: [ $a, $b, __elem_2 ], remaining: [ $($rest)* ]
        )
    }};
    (lex: $lex:ident, self_ctor: ($($ctor:tt)+), accum: [ $a:ident, $b:ident, $c:ident ], remaining: [ ($fty:ty) $($rest:tt)* ]) => {{
        if $lex.array_continue(b']')? {
            return ::core::result::Result::Err(
                $crate::Error::new($crate::ErrorKind::TypeMismatch, $lex.position()),
            );
        }
        let __elem_3 = <$fty as $crate::FromJson<'_>>::from_lex($lex)?;
        $crate::__from_json_tuple_walk!(
            lex: $lex, self_ctor: ($($ctor)+), accum: [ $a, $b, $c, __elem_3 ], remaining: [ $($rest)* ]
        )
    }};
    (lex: $lex:ident, self_ctor: ($($ctor:tt)+), accum: [ $a:ident, $b:ident, $c:ident, $d:ident ], remaining: [ ($fty:ty) $($rest:tt)* ]) => {{
        if $lex.array_continue(b']')? {
            return ::core::result::Result::Err(
                $crate::Error::new($crate::ErrorKind::TypeMismatch, $lex.position()),
            );
        }
        let __elem_4 = <$fty as $crate::FromJson<'_>>::from_lex($lex)?;
        $crate::__from_json_tuple_walk!(
            lex: $lex, self_ctor: ($($ctor)+), accum: [ $a, $b, $c, $d, __elem_4 ], remaining: [ $($rest)* ]
        )
    }};
}

// ============================================================================
// __from_json_named_body!($lex, $self_ctor, $($body:tt)*)
//
// Top-level body emitter. Drives a tt-walker that accumulates three
// parallel token streams as it consumes the field list:
//
//   - decls: per-field slot decls
//   - arms:  per-field key-dispatch arms
//   - assigns: per-field final assignments
//
// When the input is exhausted, the three streams are spliced into
// the surrounding `object_start` / `while object_next_key` / `Self {}`
// scaffold.
// ============================================================================

#[doc(hidden)]
#[macro_export]
macro_rules! __from_json_named_body {
    // Strict (default): unknown key → UnknownField error.
    ($lex:ident, ($($self_ctor:tt)+), strict, $($body:tt)*) => {
        $crate::__from_json_walk!(
            lex: $lex,
            ctor: ($($self_ctor)+),
            unknown: strict,
            decls: { },
            arms: { },
            assigns: { },
            ftokens: [],
            cur_name: (),
            cur_rename: (),
            cur_default: (),
            input: ($($body)*)
        )
    };
    // Lenient: unknown key → lex.skip_value()?.
    ($lex:ident, ($($self_ctor:tt)+), lenient, $($body:tt)*) => {
        $crate::__from_json_walk!(
            lex: $lex,
            ctor: ($($self_ctor)+),
            unknown: lenient,
            decls: { },
            arms: { },
            assigns: { },
            ftokens: [],
            cur_name: (),
            cur_rename: (),
            cur_default: (),
            input: ($($body)*)
        )
    };
}

// ============================================================================
// The tt-muncher.
//
// The state machine has these moving parts:
//
//   - `decls`, `arms`, `assigns`: append-only token lists, the three
//     parallel streams that get spliced into the final body.
//   - `ftokens`: per-field accumulator of "type tokens seen so far for
//     the current field." Cleared after each field commits.
//   - `cur_name`: the name of the field currently being collected.
//     Set when we consume `name :`, cleared when we commit.
//   - `input`: remaining tokens to consume from the user's body.
//
// Phases:
//   1. Empty input + empty cur_name → emit the body block.
//   2. We just read a field name and `:`; absorb tokens into ftokens
//      until we see `,` (commit) or input ends (commit final field).
//   3. After a comma, ftokens is empty and cur_name is empty; we expect
//      the next token to be a field name → goto phase 2.
//
// The arms below pattern-match the head of `input` to figure out which
// phase we're in.
// ============================================================================

#[doc(hidden)]
#[macro_export]
macro_rules! __from_json_walk {
    // ---------- Phase 1: terminal — input exhausted, no in-flight field. ----------
    (
        lex: $lex:ident,
        ctor: ($($self_ctor:tt)+),
        unknown: $u:ident,
        decls: { $($decls:tt)* },
        arms: { $($arms:tt)* },
        assigns: { $($assigns:tt)* },
        ftokens: [],
        cur_name: (),
        cur_rename: (),
        cur_default: (),
        input: ()
    ) => {{
        $lex.object_start()?;
        $($decls)*
        let mut __maybe_key = $lex.object_first_key_lex()?;
        while let ::core::option::Option::Some(__key_js) = __maybe_key {
            let __key_cow = $crate::key_to_cow(__key_js, $lex)?;
            match __key_cow.as_ref() {
                $($arms)*
                _ => {
                    $crate::__from_json_unknown_arm!($u, $lex);
                }
            }
            __maybe_key = $lex.object_next_key_lex()?;
        }
        ::core::result::Result::Ok($($self_ctor)+ { $($assigns)* })
    }};

    // ---------- Phase 1b-skip: terminal commit for #[bourne(skip)] field. ----------
    //
    // Skipped fields contribute nothing to `decls` or `arms`; the
    // assignment is `Default::default()`. The only thing we still
    // consume is the field's type tokens (kept on the struct def by
    // the strip walker, but here we just discard them).
    (
        lex: $lex:ident,
        ctor: ($($self_ctor:tt)+),
        unknown: $u:ident,
        decls: { $($decls:tt)* },
        arms: { $($arms:tt)* },
        assigns: { $($assigns:tt)* },
        ftokens: [ $($fty:tt)+ ],
        cur_name: ($fname:ident),
        cur_rename: ($($rename:tt)?),
        cur_default: (skip),
        input: ()
    ) => {
        $crate::__from_json_walk!(
            lex: $lex,
            ctor: ($($self_ctor)+),
            unknown: $u,
            decls: { $($decls)* },
            arms: { $($arms)* },
            assigns: {
                $($assigns)*
                $fname: <$($fty)+ as ::core::default::Default>::default(),
            },
            ftokens: [],
            cur_name: (),
            cur_rename: (),
            cur_default: (),
            input: ()
        )
    };

    // ---------- Phase 1b: terminal — last field, no trailing comma. ----------
    (
        lex: $lex:ident,
        ctor: ($($self_ctor:tt)+),
        unknown: $u:ident,
        decls: { $($decls:tt)* },
        arms: { $($arms:tt)* },
        assigns: { $($assigns:tt)* },
        ftokens: [ $($fty:tt)+ ],
        cur_name: ($fname:ident),
        cur_rename: ($($rename:tt)?),
        cur_default: ($($default:tt)?),
        input: ()
    ) => {
        $crate::__from_json_walk!(
            lex: $lex,
            ctor: ($($self_ctor)+),
            unknown: $u,
            decls: {
                $($decls)*
                let mut $fname: ::core::option::Option<$($fty)+> = ::core::option::Option::None;
            },
            arms: {
                $($arms)*
                $crate::__from_json_field_key!($fname, ($($rename)?)) => {
                    if $fname.is_some() {
                        return ::core::result::Result::Err(
                            $crate::Error::new($crate::ErrorKind::DuplicateKey, $lex.position()),
                        );
                    }
                    $fname = ::core::option::Option::Some(
                        $crate::__from_json_acquire!($lex, $($fty)+)
                    );
                }
            },
            assigns: {
                $($assigns)*
                $fname: $crate::__from_json_finalize_with_default!(
                    $lex, $fname, ($($default)?), $($fty)+
                ),
            },
            ftokens: [],
            cur_name: (),
            cur_rename: (),
            cur_default: (),
            input: ()
        )
    };

    // ---------- bourne(rename = "x") attribute on the next field. ----------
    (
        lex: $lex:ident,
        ctor: ($($self_ctor:tt)+),
        unknown: $u:ident,
        decls: $decls:tt,
        arms: $arms:tt,
        assigns: $assigns:tt,
        ftokens: [],
        cur_name: (),
        cur_rename: (),
        cur_default: ($($default:tt)?),
        input: ( #[bourne(rename = $renamed:literal)] $($rest:tt)* )
    ) => {
        $crate::__from_json_walk!(
            lex: $lex,
            ctor: ($($self_ctor)+),
            unknown: $u,
            decls: $decls,
            arms: $arms,
            assigns: $assigns,
            ftokens: [],
            cur_name: (),
            cur_rename: ($renamed),
            cur_default: ($($default)?),
            input: ($($rest)*)
        )
    };

    // ---------- bourne(skip) attribute on the next field. ----------
    //
    // A skipped field is never read from JSON. The slot decl and key
    // dispatch arm are omitted entirely; the final assignment uses
    // `Default::default()`. This is encoded as a third value on the
    // `cur_default` slot — `(skip)` — which the dedicated commit arms
    // below match before the regular `(trait_default)` / `()` arms.
    (
        lex: $lex:ident,
        ctor: ($($self_ctor:tt)+),
        unknown: $u:ident,
        decls: $decls:tt,
        arms: $arms:tt,
        assigns: $assigns:tt,
        ftokens: [],
        cur_name: (),
        cur_rename: ($($rename:tt)?),
        cur_default: (),
        input: ( #[bourne(skip)] $($rest:tt)* )
    ) => {
        $crate::__from_json_walk!(
            lex: $lex,
            ctor: ($($self_ctor)+),
            unknown: $u,
            decls: $decls,
            arms: $arms,
            assigns: $assigns,
            ftokens: [],
            cur_name: (),
            cur_rename: ($($rename)?),
            cur_default: (skip),
            input: ($($rest)*)
        )
    };

    // ---------- bourne(default) attribute on the next field. ----------
    (
        lex: $lex:ident,
        ctor: ($($self_ctor:tt)+),
        unknown: $u:ident,
        decls: $decls:tt,
        arms: $arms:tt,
        assigns: $assigns:tt,
        ftokens: [],
        cur_name: (),
        cur_rename: ($($rename:tt)?),
        cur_default: (),
        input: ( #[bourne(default)] $($rest:tt)* )
    ) => {
        $crate::__from_json_walk!(
            lex: $lex,
            ctor: ($($self_ctor)+),
            unknown: $u,
            decls: $decls,
            arms: $arms,
            assigns: $assigns,
            ftokens: [],
            cur_name: (),
            cur_rename: ($($rename)?),
            cur_default: (trait_default),
            input: ($($rest)*)
        )
    };

    // ---------- bourne(rename = "x", default) compound attribute. ----------
    (
        lex: $lex:ident,
        ctor: ($($self_ctor:tt)+),
        unknown: $u:ident,
        decls: $decls:tt,
        arms: $arms:tt,
        assigns: $assigns:tt,
        ftokens: [],
        cur_name: (),
        cur_rename: (),
        cur_default: (),
        input: ( #[bourne(rename = $renamed:literal, default)] $($rest:tt)* )
    ) => {
        $crate::__from_json_walk!(
            lex: $lex,
            ctor: ($($self_ctor)+),
            unknown: $u,
            decls: $decls,
            arms: $arms,
            assigns: $assigns,
            ftokens: [],
            cur_name: (),
            cur_rename: ($renamed),
            cur_default: (trait_default),
            input: ($($rest)*)
        )
    };

    // ---------- bourne(default = "fn") rejected with a compile_error!. ----------
    (
        lex: $lex:ident,
        ctor: ($($self_ctor:tt)+),
        unknown: $u:ident,
        decls: $decls:tt,
        arms: $arms:tt,
        assigns: $assigns:tt,
        ftokens: [],
        cur_name: (),
        cur_rename: ($($rename:tt)?),
        cur_default: (),
        input: ( #[bourne(default = $_path:literal)] $($rest:tt)* )
    ) => {
        ::core::compile_error!(
            "from_json! macro: `#[bourne(default = \"path\")]` is not supported. \
             Use bare `#[bourne(default)]` (which calls Default::default()) and implement Default \
             for the field type, or hand-write the FromJson impl. macro_rules! cannot resolve a \
             string literal to a function path."
        );
    };

    // ---------- Other `#[...]` attributes: drop them on the impl side. ----------
    // The strip-pass walker preserves them on the struct definition;
    // the impl doesn't need them.
    (
        lex: $lex:ident,
        ctor: ($($self_ctor:tt)+),
        unknown: $u:ident,
        decls: $decls:tt,
        arms: $arms:tt,
        assigns: $assigns:tt,
        ftokens: [],
        cur_name: (),
        cur_rename: ($($rename:tt)?),
        cur_default: ($($default:tt)?),
        input: ( #[$_other:meta] $($rest:tt)* )
    ) => {
        $crate::__from_json_walk!(
            lex: $lex,
            ctor: ($($self_ctor)+),
            unknown: $u,
            decls: $decls,
            arms: $arms,
            assigns: $assigns,
            ftokens: [],
            cur_name: (),
            cur_rename: ($($rename)?),
            cur_default: ($($default)?),
            input: ($($rest)*)
        )
    };

    // ---------- Phase 3: start a new field. ----------
    (
        lex: $lex:ident,
        ctor: ($($self_ctor:tt)+),
        unknown: $u:ident,
        decls: $decls:tt,
        arms: $arms:tt,
        assigns: $assigns:tt,
        ftokens: [],
        cur_name: (),
        cur_rename: ($($rename:tt)?),
        cur_default: ($($default:tt)?),
        input: ($fname:ident : $($rest:tt)*)
    ) => {
        $crate::__from_json_walk!(
            lex: $lex,
            ctor: ($($self_ctor)+),
            unknown: $u,
            decls: $decls,
            arms: $arms,
            assigns: $assigns,
            ftokens: [],
            cur_name: ($fname),
            cur_rename: ($($rename)?),
            cur_default: ($($default)?),
            input: ($($rest)*)
        )
    };

    // ---------- Phase 2-skip: commit on `,` for #[bourne(skip)] field. ----------
    (
        lex: $lex:ident,
        ctor: ($($self_ctor:tt)+),
        unknown: $u:ident,
        decls: { $($decls:tt)* },
        arms: { $($arms:tt)* },
        assigns: { $($assigns:tt)* },
        ftokens: [ $($fty:tt)+ ],
        cur_name: ($fname:ident),
        cur_rename: ($($rename:tt)?),
        cur_default: (skip),
        input: ( , $($rest:tt)* )
    ) => {
        $crate::__from_json_walk!(
            lex: $lex,
            ctor: ($($self_ctor)+),
            unknown: $u,
            decls: { $($decls)* },
            arms: { $($arms)* },
            assigns: {
                $($assigns)*
                $fname: <$($fty)+ as ::core::default::Default>::default(),
            },
            ftokens: [],
            cur_name: (),
            cur_rename: (),
            cur_default: (),
            input: ($($rest)*)
        )
    };

    // ---------- Phase 2: commit on `,`. ----------
    (
        lex: $lex:ident,
        ctor: ($($self_ctor:tt)+),
        unknown: $u:ident,
        decls: { $($decls:tt)* },
        arms: { $($arms:tt)* },
        assigns: { $($assigns:tt)* },
        ftokens: [ $($fty:tt)+ ],
        cur_name: ($fname:ident),
        cur_rename: ($($rename:tt)?),
        cur_default: ($($default:tt)?),
        input: ( , $($rest:tt)* )
    ) => {
        $crate::__from_json_walk!(
            lex: $lex,
            ctor: ($($self_ctor)+),
            unknown: $u,
            decls: {
                $($decls)*
                let mut $fname: ::core::option::Option<$($fty)+> = ::core::option::Option::None;
            },
            arms: {
                $($arms)*
                $crate::__from_json_field_key!($fname, ($($rename)?)) => {
                    if $fname.is_some() {
                        return ::core::result::Result::Err(
                            $crate::Error::new($crate::ErrorKind::DuplicateKey, $lex.position()),
                        );
                    }
                    $fname = ::core::option::Option::Some(
                        $crate::__from_json_acquire!($lex, $($fty)+)
                    );
                }
            },
            assigns: {
                $($assigns)*
                $fname: $crate::__from_json_finalize_with_default!(
                    $lex, $fname, ($($default)?), $($fty)+
                ),
            },
            ftokens: [],
            cur_name: (),
            cur_rename: (),
            cur_default: (),
            input: ($($rest)*)
        )
    };

    // ---------- Phase 2: absorb one type token. ----------
    (
        lex: $lex:ident,
        ctor: ($($self_ctor:tt)+),
        unknown: $u:ident,
        decls: $decls:tt,
        arms: $arms:tt,
        assigns: $assigns:tt,
        ftokens: [ $($fty:tt)* ],
        cur_name: ($fname:ident),
        cur_rename: ($($rename:tt)?),
        cur_default: ($($default:tt)?),
        input: ( $head:tt $($rest:tt)* )
    ) => {
        $crate::__from_json_walk!(
            lex: $lex,
            ctor: ($($self_ctor)+),
            unknown: $u,
            decls: $decls,
            arms: $arms,
            assigns: $assigns,
            ftokens: [ $($fty)* $head ],
            cur_name: ($fname),
            cur_rename: ($($rename)?),
            cur_default: ($($default)?),
            input: ($($rest)*)
        )
    };
}

// ============================================================================
// Strict vs. lenient unknown-key handling.
//
// `strict`  → raise `UnknownField` on the first unknown key.
// `lenient` → call `lex.skip_value()` to advance past the unknown
//             value (including nested arrays/objects) and continue.
// ============================================================================

#[doc(hidden)]
#[macro_export]
macro_rules! __from_json_unknown_arm {
    (strict, $lex:ident) => {
        return ::core::result::Result::Err(
            $crate::Error::new($crate::ErrorKind::UnknownField, $lex.position()),
        );
    };
    (lenient, $lex:ident) => {
        $lex.skip_value()?;
    };
}

// ============================================================================
// Per-field key resolution: rename if set, else stringified field name.
// ============================================================================

#[doc(hidden)]
#[macro_export]
macro_rules! __from_json_field_key {
    ($fname:ident, ($renamed:literal)) => { $renamed };
    ($fname:ident, ()) => { ::core::stringify!($fname) };
}

// ============================================================================
// Final-value with optional default. The 3-arg form (no default) calls
// the existing __from_json_finalize. The 4-arg form (with default)
// emits `slot.unwrap_or_else(|| <T as Default>::default())`.
// ============================================================================

#[doc(hidden)]
#[macro_export]
macro_rules! __from_json_finalize_with_default {
    // No default → existing path (Option-collapse or MissingField).
    ($lex:ident, $fname:ident, (), $($fty:tt)+) => {
        $crate::__from_json_finalize!($lex, $fname, $($fty)+)
    };
    // Bare `#[bourne(default)]` → trait Default.
    ($lex:ident, $fname:ident, (trait_default), $($fty:tt)+) => {
        $fname.unwrap_or_else(|| <$($fty)+ as ::core::default::Default>::default())
    };
}

// ============================================================================
// Per-field acquire — emits the right call for the field type.
//
// Takes the type as raw `tt`-tokens (from the `ftokens` accumulator),
// so leading-token literal matchers like `& str` work.
//
// Recognized fast paths:
//   - &str, &'lt str: parse_str_value()
//   - i8/i16/i32/i64/isize: parse_i64_value() + try_from
//   - u8/u16/u32/u64/usize: parse_i64_value() + try_from
//   - everything else: <$ty as FromJson<'_>>::from_lex
// ============================================================================

#[doc(hidden)]
#[macro_export]
macro_rules! __from_json_acquire {
    ($lex:ident, & str) => { $lex.parse_str_value()? };
    ($lex:ident, & $lt:lifetime str) => { $lex.parse_str_value()? };

    ($lex:ident, i8) => {
        <i8>::try_from($lex.parse_i64_value()?).map_err(|_| {
            $crate::Error::new($crate::ErrorKind::NumberOutOfRange, $lex.position())
        })?
    };
    ($lex:ident, i16) => {
        <i16>::try_from($lex.parse_i64_value()?).map_err(|_| {
            $crate::Error::new($crate::ErrorKind::NumberOutOfRange, $lex.position())
        })?
    };
    ($lex:ident, i32) => {
        <i32>::try_from($lex.parse_i64_value()?).map_err(|_| {
            $crate::Error::new($crate::ErrorKind::NumberOutOfRange, $lex.position())
        })?
    };
    ($lex:ident, i64) => {
        $lex.parse_i64_value()?
    };
    ($lex:ident, isize) => {
        <isize>::try_from($lex.parse_i64_value()?).map_err(|_| {
            $crate::Error::new($crate::ErrorKind::NumberOutOfRange, $lex.position())
        })?
    };

    ($lex:ident, u8) => {
        <u8>::try_from($lex.parse_i64_value()?).map_err(|_| {
            $crate::Error::new($crate::ErrorKind::NumberOutOfRange, $lex.position())
        })?
    };
    ($lex:ident, u16) => {
        <u16>::try_from($lex.parse_i64_value()?).map_err(|_| {
            $crate::Error::new($crate::ErrorKind::NumberOutOfRange, $lex.position())
        })?
    };
    ($lex:ident, u32) => {
        <u32>::try_from($lex.parse_i64_value()?).map_err(|_| {
            $crate::Error::new($crate::ErrorKind::NumberOutOfRange, $lex.position())
        })?
    };
    ($lex:ident, u64) => {
        <u64>::try_from($lex.parse_i64_value()?).map_err(|_| {
            $crate::Error::new($crate::ErrorKind::NumberOutOfRange, $lex.position())
        })?
    };
    ($lex:ident, usize) => {
        <usize>::try_from($lex.parse_i64_value()?).map_err(|_| {
            $crate::Error::new($crate::ErrorKind::NumberOutOfRange, $lex.position())
        })?
    };

    ($lex:ident, $($ty:tt)+) => {
        <$($ty)+ as $crate::FromJson<'_>>::from_lex($lex)?
    };
}

// ============================================================================
// Final-value dispatch.
//
// `Option<...>` collapses missing-key → None (the slot's outer-None,
// before unwrap, becomes inner None). Required fields raise
// MissingField if the slot was never set.
//
// The Option matcher receives the raw type tokens from ftokens, so
// the leading `Option` ident is visible literally.
// ============================================================================

// Two-step dispatch to avoid the `Option<...>` greedy-matcher
// ambiguity. The outer macro tags the field's leading token as either
// "Option" (for direct match on the literal ident) or "Other"
// (catchall), then the inner macro picks the right finalize
// expression. This avoids relying on `Option<$($_inner:tt)*>`, which
// is locally ambiguous against the catchall arm because `$_inner`
// has no fixed delimiter at its tail (`>` is just punctuation, not
// a token-tree boundary).
#[doc(hidden)]
#[macro_export]
macro_rules! __from_json_finalize {
    ($lex:ident, $fname:ident, Option < $($rest:tt)*) => {
        $crate::__from_json_finalize_option!($lex, $fname)
    };
    ($lex:ident, $fname:ident, $($fty:tt)+) => {
        $crate::__from_json_finalize_required!($lex, $fname)
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __from_json_finalize_option {
    ($lex:ident, $fname:ident) => {
        $fname.unwrap_or(::core::option::Option::None)
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __from_json_finalize_required {
    ($lex:ident, $fname:ident) => {
        $fname.ok_or_else(|| {
            $crate::Error::new($crate::ErrorKind::MissingField, $lex.position())
        })?
    };
}

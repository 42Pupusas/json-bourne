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

/// Emit a struct or enum together with its [`FromJson`](crate::FromJson) impl.
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
    // Untagged enum, no generics.
    //
    // Container attribute `#[bourne(untagged)]` parses by trial: each
    // variant is attempted in declaration order; the first one that
    // parses successfully wins. On failure the lexer is restored to
    // the value's start before the next attempt.
    //
    // JSON shapes per variant:
    //   - Unit `Foo`            → JSON `null`
    //   - Newtype `Foo(T)`      → bare `T`
    //   - Tuple `Foo(T, U)`     → `[T, U]`
    //   - Struct `Foo {a, b}`   → `{"a": ..., "b": ...}`
    //
    // Order matters: variants overlapping in shape (e.g. two newtype
    // variants where both inner types accept the same JSON) resolve
    // to the first one declared.
    // -----------------------------------------------------------------
    (
        #[bourne(untagged)]
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
                $crate::__from_json_untagged_dispatch!(__lex, $name, ($($variants)*))
            }
        }
    };

    // Untagged enum, one lifetime.
    (
        #[bourne(untagged)]
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
                $crate::__from_json_untagged_dispatch!(__lex, $name, ($($variants)*))
            }
        }
    };

    // -----------------------------------------------------------------
    // Adjacently-tagged enum, no generics.
    //
    // Container attribute `#[bourne(tag = "t", content = "c")]` puts
    // the discriminator and payload in two sibling fields:
    //
    //   {"t": "Foo", "c": <payload>}
    //   {"t": "Bar"}                  // unit — content absent
    //
    // Field order is not significant. All variant shapes are
    // supported (unit, newtype, tuple, struct), unlike the
    // internally-tagged case.
    // -----------------------------------------------------------------
    (
        #[bourne(tag = $tag:literal, content = $content:literal)]
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
                $crate::__from_json_adjacently_tagged_dispatch!(
                    __lex, $name, $tag, $content, ($($variants)*)
                )
            }
        }
    };

    // Adjacently-tagged enum, one lifetime.
    (
        #[bourne(tag = $tag:literal, content = $content:literal)]
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
                $crate::__from_json_adjacently_tagged_dispatch!(
                    __lex, $name, $tag, $content, ($($variants)*)
                )
            }
        }
    };

    // -----------------------------------------------------------------
    // Internally-tagged enum, no generics.
    //
    // Container attribute `#[bourne(tag = "type")]` makes the variant
    // discriminator a sibling field of the variant's own fields:
    //
    //   {"type": "Foo", "a": 1, "b": 2}   // → Foo { a: 1, b: 2 }
    //   {"type": "Bar"}                    // → Bar (unit)
    //
    // Container attribute must appear *first* among outer attrs (same
    // rule as `deny_unknown_fields = false`): macro_rules! arms match
    // a fixed prefix.
    //
    // Supported variant shapes: unit and struct-variant only. Newtype
    // and tuple variants do not have a coherent internally-tagged
    // representation (the discriminator would have to live inside the
    // newtype payload, which conflicts with the payload's own shape);
    // serde rejects them too.
    // -----------------------------------------------------------------
    (
        #[bourne(tag = $tag:literal)]
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
                $crate::__from_json_internally_tagged_dispatch!(
                    __lex, $name, $tag, ($($variants)*)
                )
            }
        }
    };

    // Internally-tagged enum, one lifetime.
    (
        #[bourne(tag = $tag:literal)]
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
                $crate::__from_json_internally_tagged_dispatch!(
                    __lex, $name, $tag, ($($variants)*)
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
    // bourne(skip_if_none) attribute: drop it. Ser-side only, no-op
    // for parsing (the field is just an Option<T> that defaults to
    // None when absent), but the attr has to be strippable so the
    // emitted struct def doesn't carry it.
    (
        done: $done:tt,
        kept: { $($kept:tt)* },
        cur: [ $($cur:tt)* ],
        input: ( #[bourne(skip_if_none)] $($rest:tt)* )
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
// Internally-tagged enum dispatch.
//
// Strategy: open the object, snapshot the lexer right after `{`, walk
// keys until we find the tag (parsing+restoring per non-tag value),
// read the tag's string value, then restore the snapshot and re-enter
// the variant's named-body parser with a "skip this one key by name"
// hint so the tag itself is consumed without complaint.
//
// The "find tag" pass parses non-tag values via `skip_value`, which is
// O(value-size) — for typical `{"type": "...", ...payload...}` shapes
// where the tag is the first key, that's a no-op and the second pass
// reads each value once. Worst case (tag is the last key) we read the
// payload twice. That trade is the cost of avoiding a Value DOM here.
//
// State accumulators:
//   - unit_arms:   match arms keyed on tag string → `Ok(Variant)`
//   - struct_arms: match arms keyed on tag string → struct-variant body
//                  parser invocation (with tag-skip)
// ============================================================================

#[doc(hidden)]
#[macro_export]
macro_rules! __from_json_internally_tagged_dispatch {
    ($lex:ident, $name:ident, $tag:literal, ($($variants:tt)*)) => {{
        // Snapshot the cursor before consuming `{` so the second pass
        // (variant body) can call `object_start()` again from a clean
        // state — restoring puts us back to "expect a JSON value at
        // the cursor" which is exactly what the variant body assumes.
        let __cp = $lex.checkpoint();
        $lex.object_start()?;

        // Find-tag pass. The tag value is borrowed from the input
        // slice; restoring the lexer's cursor does not invalidate
        // borrows into the input (only `offset` and stack depth move,
        // the slice itself is unchanged).
        //
        // Limitation: the tag value goes through `parse_str_value`,
        // which rejects backslash escapes. A tag like `"Foo"`
        // therefore won't match the variant name `Foo`. This matches
        // the existing struct-field-name dispatch behavior; both are
        // raised together if/when we gain a `Cow<str>` decode path
        // here.
        let mut __maybe_key = $lex.object_first_key_lex()?;
        let __tag_value: &str = loop {
            let Some(__key_js) = __maybe_key else {
                return ::core::result::Result::Err(
                    $crate::Error::new($crate::ErrorKind::MissingField, $lex.position()),
                );
            };
            let __key_cow = $crate::key_to_cow(__key_js, $lex)?;
            if __key_cow.as_ref() == $tag {
                break $lex.parse_str_value()?;
            }
            $lex.skip_value()?;
            __maybe_key = $lex.object_next_key_lex()?;
        };

        // Restore and dispatch on the tag.
        $lex.restore(__cp);
        $crate::__from_json_internally_tagged_walk!(
            lex: $lex,
            name: $name,
            tag_key: $tag,
            tag_value: (__tag_value),
            unit_arms: { },
            struct_arms: { },
            cur_rename: (),
            input: ($($variants)*)
        )
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __from_json_internally_tagged_walk {
    // Done: emit the dispatch.
    (
        lex: $lex:ident,
        name: $name:ident,
        tag_key: $tag:literal,
        tag_value: ($tagval:ident),
        unit_arms: { $($u:tt)* },
        struct_arms: { $($s:tt)* },
        cur_rename: (),
        input: ()
    ) => {
        match $tagval {
            $($u)*
            $($s)*
            _ => ::core::result::Result::Err(
                $crate::Error::new($crate::ErrorKind::UnknownField, $lex.position()),
            ),
        }
    };

    // bourne(rename = "x") on the next variant.
    (
        lex: $lex:ident,
        name: $name:ident,
        tag_key: $tag:literal,
        tag_value: ($tagval:ident),
        unit_arms: $u:tt,
        struct_arms: $s:tt,
        cur_rename: (),
        input: ( #[bourne(rename = $renamed:literal)] $($rest:tt)* )
    ) => {
        $crate::__from_json_internally_tagged_walk!(
            lex: $lex,
            name: $name,
            tag_key: $tag,
            tag_value: ($tagval),
            unit_arms: $u,
            struct_arms: $s,
            cur_rename: ($renamed),
            input: ($($rest)*)
        )
    };

    // Skip other variant attributes.
    (
        lex: $lex:ident,
        name: $name:ident,
        tag_key: $tag:literal,
        tag_value: ($tagval:ident),
        unit_arms: $u:tt,
        struct_arms: $s:tt,
        cur_rename: ($($rename:tt)?),
        input: ( #[$_other:meta] $($rest:tt)* )
    ) => {
        $crate::__from_json_internally_tagged_walk!(
            lex: $lex,
            name: $name,
            tag_key: $tag,
            tag_value: ($tagval),
            unit_arms: $u,
            struct_arms: $s,
            cur_rename: ($($rename)?),
            input: ($($rest)*)
        )
    };

    // Reject newtype variant.
    (
        lex: $lex:ident,
        name: $name:ident,
        tag_key: $tag:literal,
        tag_value: ($tagval:ident),
        unit_arms: $u:tt,
        struct_arms: $s:tt,
        cur_rename: ($($rename:tt)?),
        input: ( $vname:ident ( $($_body:tt)* ) $($rest:tt)* )
    ) => {
        ::core::compile_error!(
            "from_json! macro: internally-tagged enums (#[bourne(tag = \"...\")]) \
             do not support newtype or tuple variants. Only unit and struct variants \
             are allowed; the tag must be a sibling field of the variant's own \
             fields, which is not representable for a non-object payload."
        );
    };

    // Unit variant followed by `,`.
    (
        lex: $lex:ident,
        name: $name:ident,
        tag_key: $tag:literal,
        tag_value: ($tagval:ident),
        unit_arms: { $($u:tt)* },
        struct_arms: $s:tt,
        cur_rename: ($($rename:tt)?),
        input: ( $vname:ident , $($rest:tt)* )
    ) => {
        $crate::__from_json_internally_tagged_walk!(
            lex: $lex,
            name: $name,
            tag_key: $tag,
            tag_value: ($tagval),
            unit_arms: {
                $($u)*
                $crate::__from_json_field_key!($vname, ($($rename)?)) => {
                    $crate::__from_json_internally_tagged_unit_body!($lex, $tag);
                    ::core::result::Result::Ok($name::$vname)
                },
            },
            struct_arms: $s,
            cur_rename: (),
            input: ($($rest)*)
        )
    };
    // Unit variant, last.
    (
        lex: $lex:ident,
        name: $name:ident,
        tag_key: $tag:literal,
        tag_value: ($tagval:ident),
        unit_arms: { $($u:tt)* },
        struct_arms: $s:tt,
        cur_rename: ($($rename:tt)?),
        input: ( $vname:ident )
    ) => {
        $crate::__from_json_internally_tagged_walk!(
            lex: $lex,
            name: $name,
            tag_key: $tag,
            tag_value: ($tagval),
            unit_arms: {
                $($u)*
                $crate::__from_json_field_key!($vname, ($($rename)?)) => {
                    $crate::__from_json_internally_tagged_unit_body!($lex, $tag);
                    ::core::result::Result::Ok($name::$vname)
                },
            },
            struct_arms: $s,
            cur_rename: (),
            input: ()
        )
    };

    // Struct variant with trailing comma.
    (
        lex: $lex:ident,
        name: $name:ident,
        tag_key: $tag:literal,
        tag_value: ($tagval:ident),
        unit_arms: $u:tt,
        struct_arms: { $($s:tt)* },
        cur_rename: ($($rename:tt)?),
        input: ( $vname:ident { $($body:tt)* } , $($rest:tt)* )
    ) => {
        $crate::__from_json_internally_tagged_walk!(
            lex: $lex,
            name: $name,
            tag_key: $tag,
            tag_value: ($tagval),
            unit_arms: $u,
            struct_arms: {
                $($s)*
                $crate::__from_json_field_key!($vname, ($($rename)?)) => {
                    $crate::__from_json_named_body_skip_one!(
                        $lex,
                        ($name::$vname),
                        $tag,
                        $($body)*
                    )
                },
            },
            cur_rename: (),
            input: ($($rest)*)
        )
    };
    // Struct variant, last.
    (
        lex: $lex:ident,
        name: $name:ident,
        tag_key: $tag:literal,
        tag_value: ($tagval:ident),
        unit_arms: $u:tt,
        struct_arms: { $($s:tt)* },
        cur_rename: ($($rename:tt)?),
        input: ( $vname:ident { $($body:tt)* } )
    ) => {
        $crate::__from_json_internally_tagged_walk!(
            lex: $lex,
            name: $name,
            tag_key: $tag,
            tag_value: ($tagval),
            unit_arms: $u,
            struct_arms: {
                $($s)*
                $crate::__from_json_field_key!($vname, ($($rename)?)) => {
                    $crate::__from_json_named_body_skip_one!(
                        $lex,
                        ($name::$vname),
                        $tag,
                        $($body)*
                    )
                },
            },
            cur_rename: (),
            input: ()
        )
    };
}

// Body-walk variant of __from_json_named_body that treats one specific
// key (`$skip_key`) as the tag-and-skip-it case rather than as
// "unknown". All other keys behave strict. Reuses the existing
// `__from_json_walk!` engine via a new `unknown:` mode value
// `tag_skip(...)`.
#[doc(hidden)]
#[macro_export]
macro_rules! __from_json_named_body_skip_one {
    ($lex:ident, ($($self_ctor:tt)+), $skip_key:literal, $($body:tt)*) => {
        $crate::__from_json_walk!(
            lex: $lex,
            ctor: ($($self_ctor)+),
            unknown: (tag_skip $skip_key),
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

// Body for a unit variant in internally-tagged mode: the only other
// keys allowed are the tag itself (already validated). Any other key
// is `UnknownField`. The variant has been selected by the caller; we
// just need to drain the remaining keys.
#[doc(hidden)]
#[macro_export]
macro_rules! __from_json_internally_tagged_unit_body {
    ($lex:ident, $tag:literal) => {{
        $lex.object_start()?;
        let mut __maybe_key = $lex.object_first_key_lex()?;
        while let ::core::option::Option::Some(__key_js) = __maybe_key {
            let __key_cow = $crate::key_to_cow(__key_js, $lex)?;
            if __key_cow.as_ref() == $tag {
                $lex.skip_value()?;
            } else {
                return ::core::result::Result::Err(
                    $crate::Error::new($crate::ErrorKind::UnknownField, $lex.position()),
                );
            }
            __maybe_key = $lex.object_next_key_lex()?;
        }
    }};
}

// ============================================================================
// Adjacently-tagged enum dispatch.
//
// Walk the object once, capturing:
//   - the tag string (borrowed from input — survives `restore` since
//     restore only moves the cursor, not the input slice itself)
//   - a checkpoint pointing at the content value (if a `content` key
//     is encountered), which we use to seek back for the variant body
//
// After the object closes, dispatch on the tag. Unit-variant arms
// reject any captured content; non-unit arms require it. The restore-
// to-content step puts the lexer back into "expecting one JSON value"
// state for the variant payload.
// ============================================================================

#[doc(hidden)]
#[macro_export]
macro_rules! __from_json_adjacently_tagged_dispatch {
    ($lex:ident, $name:ident, $tag:literal, $content:literal, ($($variants:tt)*)) => {{
        $lex.object_start()?;

        let mut __tag_value: ::core::option::Option<&str> = ::core::option::Option::None;
        let mut __content_cp: ::core::option::Option<$crate::Checkpoint> =
            ::core::option::Option::None;

        let mut __maybe_key = $lex.object_first_key_lex()?;
        while let ::core::option::Option::Some(__key_js) = __maybe_key {
            let __key_cow = $crate::key_to_cow(__key_js, $lex)?;
            if __key_cow.as_ref() == $tag {
                if __tag_value.is_some() {
                    return ::core::result::Result::Err(
                        $crate::Error::new($crate::ErrorKind::DuplicateKey, $lex.position()),
                    );
                }
                __tag_value = ::core::option::Option::Some($lex.parse_str_value()?);
            } else if __key_cow.as_ref() == $content {
                if __content_cp.is_some() {
                    return ::core::result::Result::Err(
                        $crate::Error::new($crate::ErrorKind::DuplicateKey, $lex.position()),
                    );
                }
                __content_cp = ::core::option::Option::Some($lex.checkpoint());
                $lex.skip_value()?;
            } else {
                return ::core::result::Result::Err(
                    $crate::Error::new($crate::ErrorKind::UnknownField, $lex.position()),
                );
            }
            __maybe_key = $lex.object_next_key_lex()?;
        }

        let __tag = __tag_value.ok_or_else(|| {
            $crate::Error::new($crate::ErrorKind::MissingField, $lex.position())
        })?;

        // Snapshot the post-object cursor (with depth back to 0). After
        // restoring inward to the content checkpoint and parsing the
        // payload, we restore here so the outer parser sees the lexer
        // at end-of-object and `finish()` accepts the input.
        let __post_cp = $lex.checkpoint();

        let __value = $crate::__from_json_adjacently_tagged_walk!(
            lex: $lex,
            name: $name,
            tag_key: $tag,
            content_key: $content,
            tag_value: (__tag),
            content_cp: (__content_cp),
            arms: { },
            cur_rename: (),
            input: ($($variants)*)
        )?;
        $lex.restore(__post_cp);
        ::core::result::Result::Ok(__value)
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __from_json_adjacently_tagged_walk {
    // Done — emit the dispatch.
    (
        lex: $lex:ident,
        name: $name:ident,
        tag_key: $tag:literal,
        content_key: $content:literal,
        tag_value: ($tagval:ident),
        content_cp: ($cp:ident),
        arms: { $($a:tt)* },
        cur_rename: (),
        input: ()
    ) => {
        match $tagval {
            $($a)*
            _ => ::core::result::Result::Err(
                $crate::Error::new($crate::ErrorKind::UnknownField, $lex.position()),
            ),
        }
    };

    // bourne(rename = "x") on the next variant.
    (
        lex: $lex:ident,
        name: $name:ident,
        tag_key: $tag:literal,
        content_key: $content:literal,
        tag_value: ($tagval:ident),
        content_cp: ($cp:ident),
        arms: $a:tt,
        cur_rename: (),
        input: ( #[bourne(rename = $renamed:literal)] $($rest:tt)* )
    ) => {
        $crate::__from_json_adjacently_tagged_walk!(
            lex: $lex,
            name: $name,
            tag_key: $tag,
            content_key: $content,
            tag_value: ($tagval),
            content_cp: ($cp),
            arms: $a,
            cur_rename: ($renamed),
            input: ($($rest)*)
        )
    };

    // Skip other variant attributes.
    (
        lex: $lex:ident,
        name: $name:ident,
        tag_key: $tag:literal,
        content_key: $content:literal,
        tag_value: ($tagval:ident),
        content_cp: ($cp:ident),
        arms: $a:tt,
        cur_rename: ($($rename:tt)?),
        input: ( #[$_other:meta] $($rest:tt)* )
    ) => {
        $crate::__from_json_adjacently_tagged_walk!(
            lex: $lex,
            name: $name,
            tag_key: $tag,
            content_key: $content,
            tag_value: ($tagval),
            content_cp: ($cp),
            arms: $a,
            cur_rename: ($($rename)?),
            input: ($($rest)*)
        )
    };

    // Unit variant — content must NOT be present.
    (
        lex: $lex:ident,
        name: $name:ident,
        tag_key: $tag:literal,
        content_key: $content:literal,
        tag_value: ($tagval:ident),
        content_cp: ($cp:ident),
        arms: { $($a:tt)* },
        cur_rename: ($($rename:tt)?),
        input: ( $vname:ident $(, $($rest:tt)*)? )
    ) => {
        $crate::__from_json_adjacently_tagged_walk!(
            lex: $lex,
            name: $name,
            tag_key: $tag,
            content_key: $content,
            tag_value: ($tagval),
            content_cp: ($cp),
            arms: {
                $($a)*
                $crate::__from_json_field_key!($vname, ($($rename)?)) => {
                    if $cp.is_some() {
                        ::core::result::Result::Err(
                            $crate::Error::new($crate::ErrorKind::UnknownField, $lex.position()),
                        )
                    } else {
                        ::core::result::Result::Ok($name::$vname)
                    }
                },
            },
            cur_rename: (),
            input: ($($($rest)*)?)
        )
    };

    // Newtype variant — content required, parsed bare.
    (
        lex: $lex:ident,
        name: $name:ident,
        tag_key: $tag:literal,
        content_key: $content:literal,
        tag_value: ($tagval:ident),
        content_cp: ($cp:ident),
        arms: { $($a:tt)* },
        cur_rename: ($($rename:tt)?),
        input: ( $vname:ident ( $fty:ty $(,)? ) $(, $($rest:tt)*)? )
    ) => {
        $crate::__from_json_adjacently_tagged_walk!(
            lex: $lex,
            name: $name,
            tag_key: $tag,
            content_key: $content,
            tag_value: ($tagval),
            content_cp: ($cp),
            arms: {
                $($a)*
                $crate::__from_json_field_key!($vname, ($($rename)?)) => {
                    match $cp {
                        ::core::option::Option::Some(__c) => {
                            $lex.restore(__c);
                            ::core::result::Result::Ok($name::$vname(
                                <$fty as $crate::FromJson<'_>>::from_lex($lex)?,
                            ))
                        }
                        ::core::option::Option::None => ::core::result::Result::Err(
                            $crate::Error::new($crate::ErrorKind::MissingField, $lex.position()),
                        ),
                    }
                },
            },
            cur_rename: (),
            input: ($($($rest)*)?)
        )
    };

    // Multi-field tuple variant — content required, parsed as array.
    (
        lex: $lex:ident,
        name: $name:ident,
        tag_key: $tag:literal,
        content_key: $content:literal,
        tag_value: ($tagval:ident),
        content_cp: ($cp:ident),
        arms: { $($a:tt)* },
        cur_rename: ($($rename:tt)?),
        input: ( $vname:ident ( $fty1:ty, $($ftyn:ty),+ $(,)? ) $(, $($rest:tt)*)? )
    ) => {
        $crate::__from_json_adjacently_tagged_walk!(
            lex: $lex,
            name: $name,
            tag_key: $tag,
            content_key: $content,
            tag_value: ($tagval),
            content_cp: ($cp),
            arms: {
                $($a)*
                $crate::__from_json_field_key!($vname, ($($rename)?)) => {
                    match $cp {
                        ::core::option::Option::Some(__c) => {
                            $lex.restore(__c);
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
                            )
                        }
                        ::core::option::Option::None => ::core::result::Result::Err(
                            $crate::Error::new($crate::ErrorKind::MissingField, $lex.position()),
                        ),
                    }
                },
            },
            cur_rename: (),
            input: ($($($rest)*)?)
        )
    };

    // Struct variant — content required, parsed as nested object.
    (
        lex: $lex:ident,
        name: $name:ident,
        tag_key: $tag:literal,
        content_key: $content:literal,
        tag_value: ($tagval:ident),
        content_cp: ($cp:ident),
        arms: { $($a:tt)* },
        cur_rename: ($($rename:tt)?),
        input: ( $vname:ident { $($body:tt)* } $(, $($rest:tt)*)? )
    ) => {
        $crate::__from_json_adjacently_tagged_walk!(
            lex: $lex,
            name: $name,
            tag_key: $tag,
            content_key: $content,
            tag_value: ($tagval),
            content_cp: ($cp),
            arms: {
                $($a)*
                $crate::__from_json_field_key!($vname, ($($rename)?)) => {
                    match $cp {
                        ::core::option::Option::Some(__c) => {
                            $lex.restore(__c);
                            $crate::__from_json_named_body!(
                                $lex,
                                ($name::$vname),
                                strict,
                                $($body)*
                            )
                        }
                        ::core::option::Option::None => ::core::result::Result::Err(
                            $crate::Error::new($crate::ErrorKind::MissingField, $lex.position()),
                        ),
                    }
                },
            },
            cur_rename: (),
            input: ($($($rest)*)?)
        )
    };
}

// ============================================================================
// Untagged enum dispatch.
//
// Take a checkpoint at the value's start, then for each variant:
//   1. Restore (no-op on first try).
//   2. Run a `(|| { ... })()` closure that parses the variant's JSON
//      shape and constructs the variant.
//   3. On `Ok`, return.
//   4. On `Err`, fall through to the next variant.
//
// If all variants fail, return a generic TypeMismatch error pointing
// at the value's start. We cannot return all candidate errors without
// allocating; this matches serde's untagged behavior.
// ============================================================================

#[doc(hidden)]
#[macro_export]
macro_rules! __from_json_untagged_dispatch {
    ($lex:ident, $name:ident, ($($variants:tt)*)) => {{
        let __cp = $lex.checkpoint();
        $crate::__from_json_untagged_walk!(
            lex: $lex,
            name: $name,
            cp: (__cp),
            cur_rename: (),
            input: ($($variants)*)
        )
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __from_json_untagged_walk {
    // Done — all variants tried, return generic error.
    (
        lex: $lex:ident,
        name: $name:ident,
        cp: ($cp:ident),
        cur_rename: (),
        input: ()
    ) => {
        ::core::result::Result::Err(
            $crate::Error::new($crate::ErrorKind::TypeMismatch, $lex.position()),
        )
    };

    // Skip rename attribute — irrelevant to untagged (no tag string).
    (
        lex: $lex:ident,
        name: $name:ident,
        cp: ($cp:ident),
        cur_rename: (),
        input: ( #[bourne(rename = $_renamed:literal)] $($rest:tt)* )
    ) => {
        $crate::__from_json_untagged_walk!(
            lex: $lex,
            name: $name,
            cp: ($cp),
            cur_rename: (),
            input: ($($rest)*)
        )
    };

    // Skip other variant attributes.
    (
        lex: $lex:ident,
        name: $name:ident,
        cp: ($cp:ident),
        cur_rename: (),
        input: ( #[$_other:meta] $($rest:tt)* )
    ) => {
        $crate::__from_json_untagged_walk!(
            lex: $lex,
            name: $name,
            cp: ($cp),
            cur_rename: (),
            input: ($($rest)*)
        )
    };

    // Unit variant — JSON null.
    (
        lex: $lex:ident,
        name: $name:ident,
        cp: ($cp:ident),
        cur_rename: (),
        input: ( $vname:ident $(, $($rest:tt)*)? )
    ) => {{
        $lex.restore($cp);
        let __try: ::core::result::Result<$name, $crate::Error> = (|| {
            <() as $crate::FromJson<'_>>::from_lex($lex)?;
            ::core::result::Result::Ok($name::$vname)
        })();
        if let ::core::result::Result::Ok(__v) = __try {
            ::core::result::Result::Ok(__v)
        } else {
            $crate::__from_json_untagged_walk!(
                lex: $lex,
                name: $name,
                cp: ($cp),
                cur_rename: (),
                input: ($($($rest)*)?)
            )
        }
    }};

    // Newtype variant — bare inner value.
    (
        lex: $lex:ident,
        name: $name:ident,
        cp: ($cp:ident),
        cur_rename: (),
        input: ( $vname:ident ( $fty:ty $(,)? ) $(, $($rest:tt)*)? )
    ) => {{
        $lex.restore($cp);
        let __try: ::core::result::Result<$name, $crate::Error> = (|| {
            ::core::result::Result::Ok($name::$vname(
                <$fty as $crate::FromJson<'_>>::from_lex($lex)?,
            ))
        })();
        if let ::core::result::Result::Ok(__v) = __try {
            ::core::result::Result::Ok(__v)
        } else {
            $crate::__from_json_untagged_walk!(
                lex: $lex,
                name: $name,
                cp: ($cp),
                cur_rename: (),
                input: ($($($rest)*)?)
            )
        }
    }};

    // Multi-field tuple variant — JSON array.
    (
        lex: $lex:ident,
        name: $name:ident,
        cp: ($cp:ident),
        cur_rename: (),
        input: ( $vname:ident ( $fty1:ty, $($ftyn:ty),+ $(,)? ) $(, $($rest:tt)*)? )
    ) => {{
        $lex.restore($cp);
        let __try: ::core::result::Result<$name, $crate::Error> = (|| {
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
            )
        })();
        if let ::core::result::Result::Ok(__v) = __try {
            ::core::result::Result::Ok(__v)
        } else {
            $crate::__from_json_untagged_walk!(
                lex: $lex,
                name: $name,
                cp: ($cp),
                cur_rename: (),
                input: ($($($rest)*)?)
            )
        }
    }};

    // Struct variant — JSON object.
    (
        lex: $lex:ident,
        name: $name:ident,
        cp: ($cp:ident),
        cur_rename: (),
        input: ( $vname:ident { $($body:tt)* } $(, $($rest:tt)*)? )
    ) => {{
        $lex.restore($cp);
        let __try: ::core::result::Result<$name, $crate::Error> = (|| {
            $crate::__from_json_named_body!(
                $lex,
                ($name::$vname),
                strict,
                $($body)*
            )
        })();
        if let ::core::result::Result::Ok(__v) = __try {
            ::core::result::Result::Ok(__v)
        } else {
            $crate::__from_json_untagged_walk!(
                lex: $lex,
                name: $name,
                cp: ($cp),
                cur_rename: (),
                input: ($($($rest)*)?)
            )
        }
    }};
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
        unknown: $u:tt,
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
                    $crate::__from_json_unknown_arm!($u, $lex, __key_cow);
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
        unknown: $u:tt,
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
        unknown: $u:tt,
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
        unknown: $u:tt,
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
        unknown: $u:tt,
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
        unknown: $u:tt,
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

    // ---------- bourne(skip_if_none) attribute on the next field. ----------
    //
    // Pure ser-side metadata. For parsing, this is a no-op: the
    // field is an Option<T> whose missing-key case already produces
    // `None`, so we just drop the attr and continue.
    (
        lex: $lex:ident,
        ctor: ($($self_ctor:tt)+),
        unknown: $u:tt,
        decls: $decls:tt,
        arms: $arms:tt,
        assigns: $assigns:tt,
        ftokens: [],
        cur_name: (),
        cur_rename: ($($rename:tt)?),
        cur_default: ($($default:tt)?),
        input: ( #[bourne(skip_if_none)] $($rest:tt)* )
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

    // ---------- bourne(rename = "x", default) compound attribute. ----------
    (
        lex: $lex:ident,
        ctor: ($($self_ctor:tt)+),
        unknown: $u:tt,
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
        unknown: $u:tt,
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
        unknown: $u:tt,
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
        unknown: $u:tt,
        decls: $decls:tt,
        arms: $arms:tt,
        assigns: $assigns:tt,
        ftokens: [],
        cur_name: (),
        cur_rename: ($($rename:tt)?),
        cur_default: ($($default:tt)?),
        input: ($_fvis:vis $fname:ident : $($rest:tt)*)
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
        unknown: $u:tt,
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
        unknown: $u:tt,
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
        unknown: $u:tt,
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
    (strict, $lex:ident, $key:ident) => {
        return ::core::result::Result::Err(
            $crate::Error::new($crate::ErrorKind::UnknownField, $lex.position()),
        );
    };
    (lenient, $lex:ident, $key:ident) => {
        $lex.skip_value()?;
    };
    // Used by internally-tagged enum struct-variant bodies. The tag
    // key (`$skip_key`) was already consumed during the find-tag pass
    // before restore; on the second pass we encounter it again as a
    // sibling field and must skip it. Any *other* unknown key remains
    // a hard error.
    //
    // Wrapped in parens at the call site (`unknown: (tag_skip "x")`)
    // so the engine's `unknown: $u:tt` matcher captures the whole
    // mode token as a single tt.
    //
    // `$key` is the local Cow<str> bound by the body walker; passing
    // it explicitly avoids the macro_rules hygiene issue that would
    // otherwise mangle the binding when this arm expands.
    ((tag_skip $skip_key:literal), $lex:ident, $key:ident) => {
        if $key.as_ref() == $skip_key {
            $lex.skip_value()?;
        } else {
            return ::core::result::Result::Err(
                $crate::Error::new($crate::ErrorKind::UnknownField, $lex.position()),
            );
        }
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

// ============================================================================
// `to_json!` — declarative macro that emits a struct or enum together
// with its [`ToJson`] impl.
//
// Mirror image of `from_json!`. The two are independent: emitting both
// from the same source today would re-emit the type, which Rust
// disallows. Users who want round-trip macros either (a) define the
// type with `from_json!` and hand-write the `ToJson` impl, or
// (b) define with `to_json!` and hand-write `FromJson` — until a
// future combined macro lands.
//
// Field/variant attributes recognized:
//   - #[bourne(rename = "key")]     — emit that key instead of the field name
//   - #[bourne(skip)]               — omit the field from output
//   - #[bourne(skip_if_none)]       — omit Option<T> field when None
//   - #[bourne(default)]            — no-op on serialize (silently ignored)
//
// Container attributes recognized: same set as `from_json!`
// (deny_unknown_fields, untagged, tag, tag+content) — all are no-ops
// on serialize except the tagged-enum forms, which dictate variant
// encoding shape.
//
// Implementation strategy mirrors `from_json!`: tt-munch the body
// per field, accumulating a `emit:` token stream that splices into
// the impl body. The per-field state tracks (rename, skip,
// skip_if_none).
// ============================================================================

/// Emit a struct or enum together with its [`crate::ToJson`] impl.
///
/// See the [`from_json!`](crate::from_json) docs for shape coverage —
/// the two macros support the same set of containers and attributes.
#[macro_export]
macro_rules! to_json {
    // -----------------------------------------------------------------
    // Named-field struct, no generics.
    // -----------------------------------------------------------------
    (
        $(#[$attr:meta])*
        $vis:vis struct $name:ident { $($body:tt)* }
    ) => {
        $crate::__to_json_emit_struct_def!(
            attrs: { $(#[$attr])* },
            vis: $vis,
            name: $name,
            generics_def: (),
            body: ($($body)*)
        );

        impl $crate::ToJson for $name {
            fn write_json<__W: $crate::JsonWrite + ?::core::marker::Sized>(
                &self,
                __w: &mut __W,
            ) -> ::core::result::Result<(), __W::Error> {
                $crate::__to_json_named_body!(self, __w, $($body)*)
            }
        }
    };

    // -----------------------------------------------------------------
    // Named-field struct, one lifetime.
    // -----------------------------------------------------------------
    (
        $(#[$attr:meta])*
        $vis:vis struct $name:ident < $lt:lifetime $(,)? > { $($body:tt)* }
    ) => {
        $crate::__to_json_emit_struct_def!(
            attrs: { $(#[$attr])* },
            vis: $vis,
            name: $name,
            generics_def: (<$lt>),
            body: ($($body)*)
        );

        impl<$lt> $crate::ToJson for $name<$lt> {
            fn write_json<__W: $crate::JsonWrite + ?::core::marker::Sized>(
                &self,
                __w: &mut __W,
            ) -> ::core::result::Result<(), __W::Error> {
                $crate::__to_json_named_body!(self, __w, $($body)*)
            }
        }
    };

    // -----------------------------------------------------------------
    // Tuple struct, no generics, newtype (single field).
    //
    // Mirror of `from_json!`'s `#[serde(transparent)]`-style newtype:
    // serialize as a bare JSON value of the inner type, no wrapping.
    // -----------------------------------------------------------------
    (
        $(#[$attr:meta])*
        $vis:vis struct $name:ident ( $fty:ty $(,)? ) ;
    ) => {
        $(#[$attr])*
        $vis struct $name($fty);

        impl $crate::ToJson for $name {
            fn write_json<__W: $crate::JsonWrite + ?::core::marker::Sized>(
                &self,
                __w: &mut __W,
            ) -> ::core::result::Result<(), __W::Error> {
                $crate::ToJson::write_json(&self.0, __w)
            }
        }
    };

    // Tuple struct with one lifetime, newtype.
    (
        $(#[$attr:meta])*
        $vis:vis struct $name:ident < $lt:lifetime $(,)? > ( $fty:ty $(,)? ) ;
    ) => {
        $(#[$attr])*
        $vis struct $name<$lt>($fty);

        impl<$lt> $crate::ToJson for $name<$lt> {
            fn write_json<__W: $crate::JsonWrite + ?::core::marker::Sized>(
                &self,
                __w: &mut __W,
            ) -> ::core::result::Result<(), __W::Error> {
                $crate::ToJson::write_json(&self.0, __w)
            }
        }
    };

    // -----------------------------------------------------------------
    // Tuple struct, multi-field. No generics.
    //
    // Serializes as a JSON array of N elements, mirroring the
    // FromJson side. The body emits each `self.idx.write_json(w)?`
    // separated by commas.
    // -----------------------------------------------------------------
    (
        $(#[$attr:meta])*
        $vis:vis struct $name:ident ( $fty1:ty, $($ftyn:ty),+ $(,)? ) ;
    ) => {
        $(#[$attr])*
        $vis struct $name($fty1, $($ftyn),+);

        impl $crate::ToJson for $name {
            fn write_json<__W: $crate::JsonWrite + ?::core::marker::Sized>(
                &self,
                __w: &mut __W,
            ) -> ::core::result::Result<(), __W::Error> {
                __w.write_byte(b'[')?;
                $crate::ToJson::write_json(&self.0, __w)?;
                $crate::__to_json_tuple_walk!(
                    self_ref: self,
                    sink: __w,
                    idx: 1,
                    remaining: [ $(($ftyn))+ ]
                );
                __w.write_byte(b']')?;
                ::core::result::Result::Ok(())
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

        impl<$lt> $crate::ToJson for $name<$lt> {
            fn write_json<__W: $crate::JsonWrite + ?::core::marker::Sized>(
                &self,
                __w: &mut __W,
            ) -> ::core::result::Result<(), __W::Error> {
                __w.write_byte(b'[')?;
                $crate::ToJson::write_json(&self.0, __w)?;
                $crate::__to_json_tuple_walk!(
                    self_ref: self,
                    sink: __w,
                    idx: 1,
                    remaining: [ $(($ftyn))+ ]
                );
                __w.write_byte(b']')?;
                ::core::result::Result::Ok(())
            }
        }
    };

    // -----------------------------------------------------------------
    // Untagged enum, no generics.
    //
    //   #[bourne(untagged)]
    //   enum E { Foo, Bar(T), Baz(T,U), Qux { x: T } }
    //
    // emits the variant's payload bare:
    //   Foo  → null
    //   Bar  → <T>
    //   Baz  → [T, U]
    //   Qux  → {"x": T}
    //
    // The parse side disambiguates by trial; the ser side just
    // matches on `self` and writes the raw shape.
    // -----------------------------------------------------------------
    (
        #[bourne(untagged)]
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

        impl $crate::ToJson for $name {
            fn write_json<__W: $crate::JsonWrite + ?::core::marker::Sized>(
                &self,
                __w: &mut __W,
            ) -> ::core::result::Result<(), __W::Error> {
                let __this: &Self = self;
                $crate::__to_json_untagged_walk!(
                    receiver: __this,
                    sink: __w,
                    name: $name,
                    arms: { },
                    input: ($($variants)*)
                )
            }
        }
    };

    (
        #[bourne(untagged)]
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

        impl<$lt> $crate::ToJson for $name<$lt> {
            fn write_json<__W: $crate::JsonWrite + ?::core::marker::Sized>(
                &self,
                __w: &mut __W,
            ) -> ::core::result::Result<(), __W::Error> {
                let __this: &Self = self;
                $crate::__to_json_untagged_walk!(
                    receiver: __this,
                    sink: __w,
                    name: $name,
                    arms: { },
                    input: ($($variants)*)
                )
            }
        }
    };

    // -----------------------------------------------------------------
    // Internally-tagged enum, no generics.
    //
    //   #[bourne(tag = "type")]
    //   enum E { Foo, Bar { x: u32 } }
    //
    // emits { "type": "Foo" } / { "type": "Bar", "x": 42 }.
    //
    // Only unit + struct variants. Newtype / tuple variants reject at
    // parse time too — they have no coherent internally-tagged shape.
    // -----------------------------------------------------------------
    (
        #[bourne(tag = $tag:literal)]
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

        impl $crate::ToJson for $name {
            fn write_json<__W: $crate::JsonWrite + ?::core::marker::Sized>(
                &self,
                __w: &mut __W,
            ) -> ::core::result::Result<(), __W::Error> {
                let __this: &Self = self;
                $crate::__to_json_internal_walk!(
                    receiver: __this,
                    sink: __w,
                    name: $name,
                    tag: $tag,
                    arms: { },
                    cur_rename: (),
                    input: ($($variants)*)
                )
            }
        }
    };

    (
        #[bourne(tag = $tag:literal)]
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

        impl<$lt> $crate::ToJson for $name<$lt> {
            fn write_json<__W: $crate::JsonWrite + ?::core::marker::Sized>(
                &self,
                __w: &mut __W,
            ) -> ::core::result::Result<(), __W::Error> {
                let __this: &Self = self;
                $crate::__to_json_internal_walk!(
                    receiver: __this,
                    sink: __w,
                    name: $name,
                    tag: $tag,
                    arms: { },
                    cur_rename: (),
                    input: ($($variants)*)
                )
            }
        }
    };

    // -----------------------------------------------------------------
    // Adjacently-tagged enum, no generics.
    //
    //   #[bourne(tag = "t", content = "c")]
    //   enum E { Foo, Bar(u32), Baz(u32, u32), Qux { x: u32 } }
    //
    // emits:
    //   Foo  → {"t":"Foo"}
    //   Bar  → {"t":"Bar","c":42}
    //   Baz  → {"t":"Baz","c":[1,2]}
    //   Qux  → {"t":"Qux","c":{"x":1}}
    // -----------------------------------------------------------------
    (
        #[bourne(tag = $tag:literal, content = $content:literal)]
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

        impl $crate::ToJson for $name {
            fn write_json<__W: $crate::JsonWrite + ?::core::marker::Sized>(
                &self,
                __w: &mut __W,
            ) -> ::core::result::Result<(), __W::Error> {
                let __this: &Self = self;
                $crate::__to_json_adjacent_walk!(
                    receiver: __this,
                    sink: __w,
                    name: $name,
                    tag: $tag,
                    content: $content,
                    arms: { },
                    cur_rename: (),
                    input: ($($variants)*)
                )
            }
        }
    };

    (
        #[bourne(tag = $tag:literal, content = $content:literal)]
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

        impl<$lt> $crate::ToJson for $name<$lt> {
            fn write_json<__W: $crate::JsonWrite + ?::core::marker::Sized>(
                &self,
                __w: &mut __W,
            ) -> ::core::result::Result<(), __W::Error> {
                let __this: &Self = self;
                $crate::__to_json_adjacent_walk!(
                    receiver: __this,
                    sink: __w,
                    name: $name,
                    tag: $tag,
                    content: $content,
                    arms: { },
                    cur_rename: (),
                    input: ($($variants)*)
                )
            }
        }
    };

    // -----------------------------------------------------------------
    // Externally-tagged enum, no generics. (Default — no container attr.)
    //
    // Variant shapes:
    //   - Unit `Foo`            → string `"Foo"`
    //   - Newtype `Foo(T)`      → object `{"Foo": <T>}`
    //   - Tuple `Foo(T, U)`     → object `{"Foo": [<T>, <U>]}`
    //   - Struct `Foo {a, b}`   → object `{"Foo": {"a": ..., "b": ...}}`
    //
    // Variant-level `#[bourne(rename = "...")]` retags.
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

        impl $crate::ToJson for $name {
            fn write_json<__W: $crate::JsonWrite + ?::core::marker::Sized>(
                &self,
                __w: &mut __W,
            ) -> ::core::result::Result<(), __W::Error> {
                let __this: &Self = self;
                $crate::__to_json_enum_walk!(
                    receiver: __this,
                    sink: __w,
                    name: $name,
                    arms: { },
                    cur_rename: (),
                    input: ($($variants)*)
                )
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

        impl<$lt> $crate::ToJson for $name<$lt> {
            fn write_json<__W: $crate::JsonWrite + ?::core::marker::Sized>(
                &self,
                __w: &mut __W,
            ) -> ::core::result::Result<(), __W::Error> {
                let __this: &Self = self;
                $crate::__to_json_enum_walk!(
                    receiver: __this,
                    sink: __w,
                    name: $name,
                    arms: { },
                    cur_rename: (),
                    input: ($($variants)*)
                )
            }
        }
    };
}

// ============================================================================
// Externally-tagged enum walker.
//
// Emits one match arm per variant. Output is a stream of arms that
// gets spliced into a `match self { … }` at the call site.
//
// State:
//   - `arms`: append-only stream of match arms emitted so far.
//   - `cur_rename`: per-variant rename (if any). Cleared after commit.
// ============================================================================

#[doc(hidden)]
#[macro_export]
macro_rules! __to_json_enum_walk {
    // ---------- Terminal: splice arms into a match. ----------
    //
    // macro_rules! cannot expand into match arms standalone — it must
    // produce the entire match expression. The `receiver:` slot is
    // an ident the call site bound to `&Self` (avoids the `self`
    // hygiene pitfall — `self` in a macro body resolves as a module
    // path, not the method's parameter).
    //
    // `match *receiver { … }` would auto-deref but then the variant
    // patterns would need `ref __inner` to keep bindings borrowed;
    // matching `match receiver { &Variant(...) => … }` keeps the
    // bindings borrowed without `ref`. The `&` prefix on each pattern
    // below is what makes `__inner: &T` instead of `T`.
    (
        receiver: $r:ident,
        sink: $w:ident,
        name: $name:ident,
        arms: { $($arms:tt)* },
        cur_rename: (),
        input: ()
    ) => {
        match $r {
            $($arms)*
        }
    };

    // ---------- bourne(rename = "x") on the next variant. ----------
    (
        receiver: $r:ident,
        sink: $w:ident,
        name: $name:ident,
        arms: $arms:tt,
        cur_rename: (),
        input: ( #[bourne(rename = $renamed:literal)] $($rest:tt)* )
    ) => {
        $crate::__to_json_enum_walk!(
            receiver: $r,
            sink: $w,
            name: $name,
            arms: $arms,
            cur_rename: ($renamed),
            input: ($($rest)*)
        )
    };

    // ---------- Skip other attributes on the variant. ----------
    (
        receiver: $r:ident,
        sink: $w:ident,
        name: $name:ident,
        arms: $arms:tt,
        cur_rename: ($($rename:tt)?),
        input: ( #[$_other:meta] $($rest:tt)* )
    ) => {
        $crate::__to_json_enum_walk!(
            receiver: $r,
            sink: $w,
            name: $name,
            arms: $arms,
            cur_rename: ($($rename)?),
            input: ($($rest)*)
        )
    };

    // ---------- Unit variant: `VariantName,` or terminal `VariantName`. ----------
    (
        receiver: $r:ident,
        sink: $w:ident,
        name: $name:ident,
        arms: { $($arms:tt)* },
        cur_rename: ($($rename:tt)?),
        input: ( $vname:ident , $($rest:tt)* )
    ) => {
        $crate::__to_json_enum_walk!(
            receiver: $r,
            sink: $w,
            name: $name,
            arms: {
                $($arms)*
                &$name::$vname => {
                    $w.write_byte(b'"')?;
                    $w.write_str_raw($crate::__to_json_field_key!($vname, ($($rename)?)))?;
                    $w.write_byte(b'"')?;
                    ::core::result::Result::Ok(())
                }
            },
            cur_rename: (),
            input: ($($rest)*)
        )
    };
    (
        receiver: $r:ident,
        sink: $w:ident,
        name: $name:ident,
        arms: { $($arms:tt)* },
        cur_rename: ($($rename:tt)?),
        input: ( $vname:ident )
    ) => {
        $crate::__to_json_enum_walk!(
            receiver: $r,
            sink: $w,
            name: $name,
            arms: {
                $($arms)*
                &$name::$vname => {
                    $w.write_byte(b'"')?;
                    $w.write_str_raw($crate::__to_json_field_key!($vname, ($($rename)?)))?;
                    $w.write_byte(b'"')?;
                    ::core::result::Result::Ok(())
                }
            },
            cur_rename: (),
            input: ()
        )
    };

    // ---------- Newtype variant: `VariantName(T),`. ----------
    //
    // `&Self::Variant(ref __inner)` binds `__inner: &T` without
    // moving out — works for both Copy and non-Copy inner types.
    (
        receiver: $r:ident,
        sink: $w:ident,
        name: $name:ident,
        arms: { $($arms:tt)* },
        cur_rename: ($($rename:tt)?),
        input: ( $vname:ident ( $_fty:ty $(,)? ) $(, $($rest:tt)*)? )
    ) => {
        $crate::__to_json_enum_walk!(
            receiver: $r,
            sink: $w,
            name: $name,
            arms: {
                $($arms)*
                &$name::$vname(ref __inner) => {
                    $w.write_byte(b'{')?;
                    $w.write_escaped_str($crate::__to_json_field_key!($vname, ($($rename)?)))?;
                    $w.write_byte(b':')?;
                    $crate::ToJson::write_json(__inner, $w)?;
                    $w.write_byte(b'}')?;
                    ::core::result::Result::Ok(())
                }
            },
            cur_rename: (),
            input: ( $($($rest)*)? )
        )
    };

    // ---------- Tuple variant (2 fields): `VariantName(A, B),`. ----------
    (
        receiver: $r:ident,
        sink: $w:ident,
        name: $name:ident,
        arms: { $($arms:tt)* },
        cur_rename: ($($rename:tt)?),
        input: ( $vname:ident ( $_a:ty, $_b:ty $(,)? ) $(, $($rest:tt)*)? )
    ) => {
        $crate::__to_json_enum_walk!(
            receiver: $r,
            sink: $w,
            name: $name,
            arms: {
                $($arms)*
                &$name::$vname(ref __a, ref __b) => {
                    $w.write_byte(b'{')?;
                    $w.write_escaped_str($crate::__to_json_field_key!($vname, ($($rename)?)))?;
                    $w.write_byte(b':')?;
                    $w.write_byte(b'[')?;
                    $crate::ToJson::write_json(__a, $w)?;
                    $w.write_byte(b',')?;
                    $crate::ToJson::write_json(__b, $w)?;
                    $w.write_byte(b']')?;
                    $w.write_byte(b'}')?;
                    ::core::result::Result::Ok(())
                }
            },
            cur_rename: (),
            input: ( $($($rest)*)? )
        )
    };

    // ---------- Tuple variant (3 fields). ----------
    (
        receiver: $r:ident,
        sink: $w:ident,
        name: $name:ident,
        arms: { $($arms:tt)* },
        cur_rename: ($($rename:tt)?),
        input: ( $vname:ident ( $_a:ty, $_b:ty, $_c:ty $(,)? ) $(, $($rest:tt)*)? )
    ) => {
        $crate::__to_json_enum_walk!(
            receiver: $r,
            sink: $w,
            name: $name,
            arms: {
                $($arms)*
                &$name::$vname(ref __a, ref __b, ref __c) => {
                    $w.write_byte(b'{')?;
                    $w.write_escaped_str($crate::__to_json_field_key!($vname, ($($rename)?)))?;
                    $w.write_byte(b':')?;
                    $w.write_byte(b'[')?;
                    $crate::ToJson::write_json(__a, $w)?;
                    $w.write_byte(b',')?;
                    $crate::ToJson::write_json(__b, $w)?;
                    $w.write_byte(b',')?;
                    $crate::ToJson::write_json(__c, $w)?;
                    $w.write_byte(b']')?;
                    $w.write_byte(b'}')?;
                    ::core::result::Result::Ok(())
                }
            },
            cur_rename: (),
            input: ( $($($rest)*)? )
        )
    };

    // ---------- Struct variant: `VariantName { a: A, b: B },`. ----------
    (
        receiver: $r:ident,
        sink: $w:ident,
        name: $name:ident,
        arms: { $($arms:tt)* },
        cur_rename: ($($rename:tt)?),
        input: ( $vname:ident { $($fname:ident : $_fty:ty),+ $(,)? } $(, $($rest:tt)*)? )
    ) => {
        $crate::__to_json_enum_walk!(
            receiver: $r,
            sink: $w,
            name: $name,
            arms: {
                $($arms)*
                &$name::$vname { $(ref $fname),+ } => {
                    $w.write_byte(b'{')?;
                    $w.write_escaped_str($crate::__to_json_field_key!($vname, ($($rename)?)))?;
                    $w.write_str_raw(":{")?;
                    // Per-field write fused into a single &'static str.
                    // `concat!` evaluates at expand time so each field
                    // emits one `write_str_raw` (one `String::push_str`)
                    // for the comma+`"key":` punctuation, replacing what
                    // used to be five sink calls. The runtime `__first`
                    // branch stays — macro_rules! can't peel the first
                    // field of a `$(...)+ ` repetition.
                    let mut __first: bool = true;
                    $(
                        if !__first { $w.write_byte(b',')?; }
                        $w.write_str_raw(
                            ::core::concat!("\"", ::core::stringify!($fname), "\":")
                        )?;
                        $crate::ToJson::write_json($fname, $w)?;
                        __first = false;
                    )+
                    $w.write_str_raw("}}")?;
                    ::core::result::Result::Ok(())
                }
            },
            cur_rename: (),
            input: ( $($($rest)*)? )
        )
    };
}

// ============================================================================
// Internally-tagged enum walker.
//
// Each variant is encoded as a single object whose first key is the
// configured tag literal carrying the variant name (or rename), and
// whose remaining keys are the variant's struct fields. Unit variants
// produce just the tag entry.
//
// Newtype / tuple variants are unsupported (mirror from_json! — they
// have no coherent internally-tagged shape). The macro is silently
// silent if you try to use one; it just won't match any arm and
// you'll get a compilation error pointing at the variant. That's
// the same failure mode as the parse side.
// ============================================================================

#[doc(hidden)]
#[macro_export]
macro_rules! __to_json_internal_walk {
    // Terminal: splice arms into a match.
    (
        receiver: $r:ident,
        sink: $w:ident,
        name: $name:ident,
        tag: $tag:literal,
        arms: { $($arms:tt)* },
        cur_rename: (),
        input: ()
    ) => {
        match $r {
            $($arms)*
        }
    };

    // bourne(rename = "x") on the next variant.
    (
        receiver: $r:ident,
        sink: $w:ident,
        name: $name:ident,
        tag: $tag:literal,
        arms: $arms:tt,
        cur_rename: (),
        input: ( #[bourne(rename = $renamed:literal)] $($rest:tt)* )
    ) => {
        $crate::__to_json_internal_walk!(
            receiver: $r,
            sink: $w,
            name: $name,
            tag: $tag,
            arms: $arms,
            cur_rename: ($renamed),
            input: ($($rest)*)
        )
    };

    // Skip other attributes.
    (
        receiver: $r:ident,
        sink: $w:ident,
        name: $name:ident,
        tag: $tag:literal,
        arms: $arms:tt,
        cur_rename: ($($rename:tt)?),
        input: ( #[$_other:meta] $($rest:tt)* )
    ) => {
        $crate::__to_json_internal_walk!(
            receiver: $r,
            sink: $w,
            name: $name,
            tag: $tag,
            arms: $arms,
            cur_rename: ($($rename)?),
            input: ($($rest)*)
        )
    };

    // Unit variant: `{"tag":"VariantName"}`.
    (
        receiver: $r:ident,
        sink: $w:ident,
        name: $name:ident,
        tag: $tag:literal,
        arms: { $($arms:tt)* },
        cur_rename: ($($rename:tt)?),
        input: ( $vname:ident , $($rest:tt)* )
    ) => {
        $crate::__to_json_internal_walk!(
            receiver: $r,
            sink: $w,
            name: $name,
            tag: $tag,
            arms: {
                $($arms)*
                &$name::$vname => {
                    $w.write_byte(b'{')?;
                    $w.write_escaped_str($tag)?;
                    $w.write_byte(b':')?;
                    $w.write_byte(b'"')?;
                    $w.write_str_raw($crate::__to_json_field_key!($vname, ($($rename)?)))?;
                    $w.write_byte(b'"')?;
                    $w.write_byte(b'}')?;
                    ::core::result::Result::Ok(())
                }
            },
            cur_rename: (),
            input: ($($rest)*)
        )
    };
    (
        receiver: $r:ident,
        sink: $w:ident,
        name: $name:ident,
        tag: $tag:literal,
        arms: { $($arms:tt)* },
        cur_rename: ($($rename:tt)?),
        input: ( $vname:ident )
    ) => {
        $crate::__to_json_internal_walk!(
            receiver: $r,
            sink: $w,
            name: $name,
            tag: $tag,
            arms: {
                $($arms)*
                &$name::$vname => {
                    $w.write_byte(b'{')?;
                    $w.write_escaped_str($tag)?;
                    $w.write_byte(b':')?;
                    $w.write_byte(b'"')?;
                    $w.write_str_raw($crate::__to_json_field_key!($vname, ($($rename)?)))?;
                    $w.write_byte(b'"')?;
                    $w.write_byte(b'}')?;
                    ::core::result::Result::Ok(())
                }
            },
            cur_rename: (),
            input: ()
        )
    };

    // Struct variant: `{"tag":"VariantName","field1":..,"field2":..}`.
    (
        receiver: $r:ident,
        sink: $w:ident,
        name: $name:ident,
        tag: $tag:literal,
        arms: { $($arms:tt)* },
        cur_rename: ($($rename:tt)?),
        input: ( $vname:ident { $($fname:ident : $_fty:ty),+ $(,)? } $(, $($rest:tt)*)? )
    ) => {
        $crate::__to_json_internal_walk!(
            receiver: $r,
            sink: $w,
            name: $name,
            tag: $tag,
            arms: {
                $($arms)*
                &$name::$vname { $(ref $fname),+ } => {
                    $w.write_byte(b'{')?;
                    $w.write_escaped_str($tag)?;
                    $w.write_byte(b':')?;
                    $w.write_byte(b'"')?;
                    $w.write_str_raw($crate::__to_json_field_key!($vname, ($($rename)?)))?;
                    $w.write_byte(b'"')?;
                    // Tag is always the first key, so every struct
                    // field unconditionally needs the leading comma —
                    // no `__first` flag required. Fuse `,"key":` into
                    // a single `concat!` literal per field.
                    $(
                        $w.write_str_raw(
                            ::core::concat!(",\"", ::core::stringify!($fname), "\":")
                        )?;
                        $crate::ToJson::write_json($fname, $w)?;
                    )+
                    $w.write_byte(b'}')?;
                    ::core::result::Result::Ok(())
                }
            },
            cur_rename: (),
            input: ( $($($rest)*)? )
        )
    };
}

// ============================================================================
// Adjacently-tagged enum walker.
//
// Each variant emits {"tag":"Name","content":<payload>}. Unit
// variants omit the content key entirely (mirror the parse side).
// All four variant shapes are supported.
// ============================================================================

#[doc(hidden)]
#[macro_export]
macro_rules! __to_json_adjacent_walk {
    // Terminal.
    (
        receiver: $r:ident,
        sink: $w:ident,
        name: $name:ident,
        tag: $tag:literal,
        content: $content:literal,
        arms: { $($arms:tt)* },
        cur_rename: (),
        input: ()
    ) => {
        match $r {
            $($arms)*
        }
    };

    // bourne(rename = "x").
    (
        receiver: $r:ident,
        sink: $w:ident,
        name: $name:ident,
        tag: $tag:literal,
        content: $content:literal,
        arms: $arms:tt,
        cur_rename: (),
        input: ( #[bourne(rename = $renamed:literal)] $($rest:tt)* )
    ) => {
        $crate::__to_json_adjacent_walk!(
            receiver: $r,
            sink: $w,
            name: $name,
            tag: $tag,
            content: $content,
            arms: $arms,
            cur_rename: ($renamed),
            input: ($($rest)*)
        )
    };

    // Skip other attributes.
    (
        receiver: $r:ident,
        sink: $w:ident,
        name: $name:ident,
        tag: $tag:literal,
        content: $content:literal,
        arms: $arms:tt,
        cur_rename: ($($rename:tt)?),
        input: ( #[$_other:meta] $($rest:tt)* )
    ) => {
        $crate::__to_json_adjacent_walk!(
            receiver: $r,
            sink: $w,
            name: $name,
            tag: $tag,
            content: $content,
            arms: $arms,
            cur_rename: ($($rename)?),
            input: ($($rest)*)
        )
    };

    // Unit variant: `{"tag":"Name"}` — no content key.
    (
        receiver: $r:ident,
        sink: $w:ident,
        name: $name:ident,
        tag: $tag:literal,
        content: $content:literal,
        arms: { $($arms:tt)* },
        cur_rename: ($($rename:tt)?),
        input: ( $vname:ident , $($rest:tt)* )
    ) => {
        $crate::__to_json_adjacent_walk!(
            receiver: $r,
            sink: $w,
            name: $name,
            tag: $tag,
            content: $content,
            arms: {
                $($arms)*
                &$name::$vname => {
                    $w.write_byte(b'{')?;
                    $w.write_escaped_str($tag)?;
                    $w.write_byte(b':')?;
                    $w.write_byte(b'"')?;
                    $w.write_str_raw($crate::__to_json_field_key!($vname, ($($rename)?)))?;
                    $w.write_byte(b'"')?;
                    $w.write_byte(b'}')?;
                    ::core::result::Result::Ok(())
                }
            },
            cur_rename: (),
            input: ($($rest)*)
        )
    };
    (
        receiver: $r:ident,
        sink: $w:ident,
        name: $name:ident,
        tag: $tag:literal,
        content: $content:literal,
        arms: { $($arms:tt)* },
        cur_rename: ($($rename:tt)?),
        input: ( $vname:ident )
    ) => {
        $crate::__to_json_adjacent_walk!(
            receiver: $r,
            sink: $w,
            name: $name,
            tag: $tag,
            content: $content,
            arms: {
                $($arms)*
                &$name::$vname => {
                    $w.write_byte(b'{')?;
                    $w.write_escaped_str($tag)?;
                    $w.write_byte(b':')?;
                    $w.write_byte(b'"')?;
                    $w.write_str_raw($crate::__to_json_field_key!($vname, ($($rename)?)))?;
                    $w.write_byte(b'"')?;
                    $w.write_byte(b'}')?;
                    ::core::result::Result::Ok(())
                }
            },
            cur_rename: (),
            input: ()
        )
    };

    // Newtype variant: `{"tag":"Name","content":<inner>}`.
    (
        receiver: $r:ident,
        sink: $w:ident,
        name: $name:ident,
        tag: $tag:literal,
        content: $content:literal,
        arms: { $($arms:tt)* },
        cur_rename: ($($rename:tt)?),
        input: ( $vname:ident ( $_fty:ty $(,)? ) $(, $($rest:tt)*)? )
    ) => {
        $crate::__to_json_adjacent_walk!(
            receiver: $r,
            sink: $w,
            name: $name,
            tag: $tag,
            content: $content,
            arms: {
                $($arms)*
                &$name::$vname(ref __inner) => {
                    $w.write_byte(b'{')?;
                    $w.write_escaped_str($tag)?;
                    $w.write_byte(b':')?;
                    $w.write_byte(b'"')?;
                    $w.write_str_raw($crate::__to_json_field_key!($vname, ($($rename)?)))?;
                    $w.write_byte(b'"')?;
                    $w.write_byte(b',')?;
                    $w.write_escaped_str($content)?;
                    $w.write_byte(b':')?;
                    $crate::ToJson::write_json(__inner, $w)?;
                    $w.write_byte(b'}')?;
                    ::core::result::Result::Ok(())
                }
            },
            cur_rename: (),
            input: ( $($($rest)*)? )
        )
    };

    // Tuple variant (2 fields): content is a JSON array.
    (
        receiver: $r:ident,
        sink: $w:ident,
        name: $name:ident,
        tag: $tag:literal,
        content: $content:literal,
        arms: { $($arms:tt)* },
        cur_rename: ($($rename:tt)?),
        input: ( $vname:ident ( $_a:ty, $_b:ty $(,)? ) $(, $($rest:tt)*)? )
    ) => {
        $crate::__to_json_adjacent_walk!(
            receiver: $r,
            sink: $w,
            name: $name,
            tag: $tag,
            content: $content,
            arms: {
                $($arms)*
                &$name::$vname(ref __a, ref __b) => {
                    $w.write_byte(b'{')?;
                    $w.write_escaped_str($tag)?;
                    $w.write_byte(b':')?;
                    $w.write_byte(b'"')?;
                    $w.write_str_raw($crate::__to_json_field_key!($vname, ($($rename)?)))?;
                    $w.write_byte(b'"')?;
                    $w.write_byte(b',')?;
                    $w.write_escaped_str($content)?;
                    $w.write_byte(b':')?;
                    $w.write_byte(b'[')?;
                    $crate::ToJson::write_json(__a, $w)?;
                    $w.write_byte(b',')?;
                    $crate::ToJson::write_json(__b, $w)?;
                    $w.write_byte(b']')?;
                    $w.write_byte(b'}')?;
                    ::core::result::Result::Ok(())
                }
            },
            cur_rename: (),
            input: ( $($($rest)*)? )
        )
    };

    // Tuple variant (3 fields).
    (
        receiver: $r:ident,
        sink: $w:ident,
        name: $name:ident,
        tag: $tag:literal,
        content: $content:literal,
        arms: { $($arms:tt)* },
        cur_rename: ($($rename:tt)?),
        input: ( $vname:ident ( $_a:ty, $_b:ty, $_c:ty $(,)? ) $(, $($rest:tt)*)? )
    ) => {
        $crate::__to_json_adjacent_walk!(
            receiver: $r,
            sink: $w,
            name: $name,
            tag: $tag,
            content: $content,
            arms: {
                $($arms)*
                &$name::$vname(ref __a, ref __b, ref __c) => {
                    $w.write_byte(b'{')?;
                    $w.write_escaped_str($tag)?;
                    $w.write_byte(b':')?;
                    $w.write_byte(b'"')?;
                    $w.write_str_raw($crate::__to_json_field_key!($vname, ($($rename)?)))?;
                    $w.write_byte(b'"')?;
                    $w.write_byte(b',')?;
                    $w.write_escaped_str($content)?;
                    $w.write_byte(b':')?;
                    $w.write_byte(b'[')?;
                    $crate::ToJson::write_json(__a, $w)?;
                    $w.write_byte(b',')?;
                    $crate::ToJson::write_json(__b, $w)?;
                    $w.write_byte(b',')?;
                    $crate::ToJson::write_json(__c, $w)?;
                    $w.write_byte(b']')?;
                    $w.write_byte(b'}')?;
                    ::core::result::Result::Ok(())
                }
            },
            cur_rename: (),
            input: ( $($($rest)*)? )
        )
    };

    // Struct variant: content is a JSON object.
    (
        receiver: $r:ident,
        sink: $w:ident,
        name: $name:ident,
        tag: $tag:literal,
        content: $content:literal,
        arms: { $($arms:tt)* },
        cur_rename: ($($rename:tt)?),
        input: ( $vname:ident { $($fname:ident : $_fty:ty),+ $(,)? } $(, $($rest:tt)*)? )
    ) => {
        $crate::__to_json_adjacent_walk!(
            receiver: $r,
            sink: $w,
            name: $name,
            tag: $tag,
            content: $content,
            arms: {
                $($arms)*
                &$name::$vname { $(ref $fname),+ } => {
                    $w.write_byte(b'{')?;
                    $w.write_escaped_str($tag)?;
                    $w.write_byte(b':')?;
                    $w.write_byte(b'"')?;
                    $w.write_str_raw($crate::__to_json_field_key!($vname, ($($rename)?)))?;
                    $w.write_byte(b'"')?;
                    $w.write_byte(b',')?;
                    $w.write_escaped_str($content)?;
                    $w.write_str_raw(":{")?;
                    let mut __first: bool = true;
                    $(
                        if !__first { $w.write_byte(b',')?; }
                        $w.write_str_raw(
                            ::core::concat!("\"", ::core::stringify!($fname), "\":")
                        )?;
                        $crate::ToJson::write_json($fname, $w)?;
                        __first = false;
                    )+
                    $w.write_str_raw("}}")?;
                    ::core::result::Result::Ok(())
                }
            },
            cur_rename: (),
            input: ( $($($rest)*)? )
        )
    };
}

// ============================================================================
// Untagged enum walker.
//
// Each variant's payload is written bare with no surrounding tag.
// Unit → `null`, newtype → inner, tuple → array, struct → object.
// Variant-level rename has no meaning (no tag is emitted), so we
// simply ignore it.
// ============================================================================

#[doc(hidden)]
#[macro_export]
macro_rules! __to_json_untagged_walk {
    // Terminal.
    (
        receiver: $r:ident,
        sink: $w:ident,
        name: $name:ident,
        arms: { $($arms:tt)* },
        input: ()
    ) => {
        match $r {
            $($arms)*
        }
    };

    // Drop attributes — rename has no tag to retag here, ignore quietly.
    (
        receiver: $r:ident,
        sink: $w:ident,
        name: $name:ident,
        arms: $arms:tt,
        input: ( #[$_other:meta] $($rest:tt)* )
    ) => {
        $crate::__to_json_untagged_walk!(
            receiver: $r,
            sink: $w,
            name: $name,
            arms: $arms,
            input: ($($rest)*)
        )
    };

    // Unit variant: emit `null`.
    (
        receiver: $r:ident,
        sink: $w:ident,
        name: $name:ident,
        arms: { $($arms:tt)* },
        input: ( $vname:ident , $($rest:tt)* )
    ) => {
        $crate::__to_json_untagged_walk!(
            receiver: $r,
            sink: $w,
            name: $name,
            arms: {
                $($arms)*
                &$name::$vname => {
                    $w.write_str_raw("null")?;
                    ::core::result::Result::Ok(())
                }
            },
            input: ($($rest)*)
        )
    };
    (
        receiver: $r:ident,
        sink: $w:ident,
        name: $name:ident,
        arms: { $($arms:tt)* },
        input: ( $vname:ident )
    ) => {
        $crate::__to_json_untagged_walk!(
            receiver: $r,
            sink: $w,
            name: $name,
            arms: {
                $($arms)*
                &$name::$vname => {
                    $w.write_str_raw("null")?;
                    ::core::result::Result::Ok(())
                }
            },
            input: ()
        )
    };

    // Newtype variant: emit the inner value bare.
    (
        receiver: $r:ident,
        sink: $w:ident,
        name: $name:ident,
        arms: { $($arms:tt)* },
        input: ( $vname:ident ( $_fty:ty $(,)? ) $(, $($rest:tt)*)? )
    ) => {
        $crate::__to_json_untagged_walk!(
            receiver: $r,
            sink: $w,
            name: $name,
            arms: {
                $($arms)*
                &$name::$vname(ref __inner) => {
                    $crate::ToJson::write_json(__inner, $w)?;
                    ::core::result::Result::Ok(())
                }
            },
            input: ( $($($rest)*)? )
        )
    };

    // Tuple variant (2 fields): emit as array.
    (
        receiver: $r:ident,
        sink: $w:ident,
        name: $name:ident,
        arms: { $($arms:tt)* },
        input: ( $vname:ident ( $_a:ty, $_b:ty $(,)? ) $(, $($rest:tt)*)? )
    ) => {
        $crate::__to_json_untagged_walk!(
            receiver: $r,
            sink: $w,
            name: $name,
            arms: {
                $($arms)*
                &$name::$vname(ref __a, ref __b) => {
                    $w.write_byte(b'[')?;
                    $crate::ToJson::write_json(__a, $w)?;
                    $w.write_byte(b',')?;
                    $crate::ToJson::write_json(__b, $w)?;
                    $w.write_byte(b']')?;
                    ::core::result::Result::Ok(())
                }
            },
            input: ( $($($rest)*)? )
        )
    };

    // Tuple variant (3 fields).
    (
        receiver: $r:ident,
        sink: $w:ident,
        name: $name:ident,
        arms: { $($arms:tt)* },
        input: ( $vname:ident ( $_a:ty, $_b:ty, $_c:ty $(,)? ) $(, $($rest:tt)*)? )
    ) => {
        $crate::__to_json_untagged_walk!(
            receiver: $r,
            sink: $w,
            name: $name,
            arms: {
                $($arms)*
                &$name::$vname(ref __a, ref __b, ref __c) => {
                    $w.write_byte(b'[')?;
                    $crate::ToJson::write_json(__a, $w)?;
                    $w.write_byte(b',')?;
                    $crate::ToJson::write_json(__b, $w)?;
                    $w.write_byte(b',')?;
                    $crate::ToJson::write_json(__c, $w)?;
                    $w.write_byte(b']')?;
                    ::core::result::Result::Ok(())
                }
            },
            input: ( $($($rest)*)? )
        )
    };

    // Struct variant: emit as object.
    (
        receiver: $r:ident,
        sink: $w:ident,
        name: $name:ident,
        arms: { $($arms:tt)* },
        input: ( $vname:ident { $($fname:ident : $_fty:ty),+ $(,)? } $(, $($rest:tt)*)? )
    ) => {
        $crate::__to_json_untagged_walk!(
            receiver: $r,
            sink: $w,
            name: $name,
            arms: {
                $($arms)*
                &$name::$vname { $(ref $fname),+ } => {
                    $w.write_byte(b'{')?;
                    let mut __first: bool = true;
                    $(
                        if !__first { $w.write_byte(b',')?; }
                        $w.write_str_raw(
                            ::core::concat!("\"", ::core::stringify!($fname), "\":")
                        )?;
                        $crate::ToJson::write_json($fname, $w)?;
                        __first = false;
                    )+
                    $w.write_byte(b'}')?;
                    ::core::result::Result::Ok(())
                }
            },
            input: ( $($($rest)*)? )
        )
    };
}

// ============================================================================
// Tuple-walk: emit `, self.IDX.write_json(w)?;` for each remaining
// field, incrementing IDX. Mirrors `__from_json_tuple_walk` on the
// parse side.
//
// macro_rules! has no integer arithmetic, so the index has to be a
// separate ident at each call site. We emit numeric idents (1, 2, 3,
// ...) by hand: the macro takes an explicit `idx:` head and recurses
// with the next number. A helper for arities up to 6 is enough for
// every shape `from_json!` supports.
// ============================================================================

#[doc(hidden)]
#[macro_export]
macro_rules! __to_json_tuple_walk {
    // Terminal: nothing left.
    (
        self_ref: $self:ident,
        sink: $w:ident,
        idx: $_idx:tt,
        remaining: []
    ) => {};

    // 1 field remaining: emit it at $idx.
    (
        self_ref: $self:ident,
        sink: $w:ident,
        idx: 1,
        remaining: [ ($_fty:ty) ]
    ) => {
        $w.write_byte(b',')?;
        $crate::ToJson::write_json(&$self.1, $w)?;
    };
    (
        self_ref: $self:ident,
        sink: $w:ident,
        idx: 2,
        remaining: [ ($_fty:ty) ]
    ) => {
        $w.write_byte(b',')?;
        $crate::ToJson::write_json(&$self.2, $w)?;
    };
    (
        self_ref: $self:ident,
        sink: $w:ident,
        idx: 3,
        remaining: [ ($_fty:ty) ]
    ) => {
        $w.write_byte(b',')?;
        $crate::ToJson::write_json(&$self.3, $w)?;
    };
    (
        self_ref: $self:ident,
        sink: $w:ident,
        idx: 4,
        remaining: [ ($_fty:ty) ]
    ) => {
        $w.write_byte(b',')?;
        $crate::ToJson::write_json(&$self.4, $w)?;
    };
    (
        self_ref: $self:ident,
        sink: $w:ident,
        idx: 5,
        remaining: [ ($_fty:ty) ]
    ) => {
        $w.write_byte(b',')?;
        $crate::ToJson::write_json(&$self.5, $w)?;
    };

    // 2+ fields remaining at idx 1: emit and recurse.
    (
        self_ref: $self:ident,
        sink: $w:ident,
        idx: 1,
        remaining: [ ($_fty:ty) $(($rest:ty))+ ]
    ) => {
        $w.write_byte(b',')?;
        $crate::ToJson::write_json(&$self.1, $w)?;
        $crate::__to_json_tuple_walk!(
            self_ref: $self,
            sink: $w,
            idx: 2,
            remaining: [ $(($rest))+ ]
        )
    };
    (
        self_ref: $self:ident,
        sink: $w:ident,
        idx: 2,
        remaining: [ ($_fty:ty) $(($rest:ty))+ ]
    ) => {
        $w.write_byte(b',')?;
        $crate::ToJson::write_json(&$self.2, $w)?;
        $crate::__to_json_tuple_walk!(
            self_ref: $self,
            sink: $w,
            idx: 3,
            remaining: [ $(($rest))+ ]
        )
    };
    (
        self_ref: $self:ident,
        sink: $w:ident,
        idx: 3,
        remaining: [ ($_fty:ty) $(($rest:ty))+ ]
    ) => {
        $w.write_byte(b',')?;
        $crate::ToJson::write_json(&$self.3, $w)?;
        $crate::__to_json_tuple_walk!(
            self_ref: $self,
            sink: $w,
            idx: 4,
            remaining: [ $(($rest))+ ]
        )
    };
    (
        self_ref: $self:ident,
        sink: $w:ident,
        idx: 4,
        remaining: [ ($_fty:ty) $(($rest:ty))+ ]
    ) => {
        $w.write_byte(b',')?;
        $crate::ToJson::write_json(&$self.4, $w)?;
        $crate::__to_json_tuple_walk!(
            self_ref: $self,
            sink: $w,
            idx: 5,
            remaining: [ $(($rest))+ ]
        )
    };
}

// ============================================================================
// Struct-definition emitter for `to_json!`.
//
// Identical structure to `__from_json_emit_struct_def` — strips
// `#[bourne(...)]` field attrs and re-emits the clean type. The
// attribute strip walker is shared via `__from_json_strip_walk` since
// the strip rules are the same (rename, skip, default, skip_if_none
// all get dropped from the emitted def).
//
// `skip_if_none` is new on the ser side; we add a strip arm for it
// inside `__from_json_strip_walk` further down.
// ============================================================================

#[doc(hidden)]
#[macro_export]
macro_rules! __to_json_emit_struct_def {
    (
        attrs: { $(#[$attr:meta])* },
        vis: $vis:vis,
        name: $name:ident,
        generics_def: $gen:tt,
        body: ($($body:tt)*)
    ) => {
        // Reuse the from_json strip walker. The output is identical:
        // a clean struct def with bourne-attrs stripped and every
        // other attr (incl. user-written ones) preserved.
        $crate::__from_json_emit_struct_def!(
            attrs: { $(#[$attr])* },
            vis: $vis,
            name: $name,
            generics_def: $gen,
            body: ($($body)*)
        );
    };
}

// ============================================================================
// `__to_json_named_body` — emits the struct-impl body.
//
// The body shape:
//
//   __w.write_byte(b'{')?;
//   let mut __first: bool = true;
//   <per-field emit>
//   __w.write_byte(b'}')?;
//   ::core::result::Result::Ok(())
//
// Per-field emit (no skip_if_none):
//
//   if !__first { __w.write_byte(b',')?; }
//   __w.write_escaped_str("key")?;
//   __w.write_byte(b':')?;
//   $crate::ToJson::write_json(&self.fname, __w)?;
//   __first = false;
//
// Per-field emit (skip_if_none, only valid on Option<T>):
//
//   if let ::core::option::Option::Some(ref __v) = self.fname {
//       if !__first { __w.write_byte(b',')?; }
//       __w.write_escaped_str("key")?;
//       __w.write_byte(b':')?;
//       $crate::ToJson::write_json(__v, __w)?;
//       __first = false;
//   }
//
// `__first` carries the "have we emitted any field yet" state, which
// is the cleanest way to handle dynamically-skipped fields. Constant
// folding will eliminate the `if !__first` branch in the first
// iteration; the rest is one cmp+branch per field, dwarfed by the
// actual ToJson::write_json call.
// ============================================================================

#[doc(hidden)]
#[macro_export]
macro_rules! __to_json_named_body {
    ($self:ident, $w:ident, $($body:tt)*) => {{
        $w.write_byte(b'{')?;
        // `__first` exists only for the dynamic-comma path: as soon as
        // a `skip_if_none` field is encountered, comma placement
        // becomes runtime-dependent and subsequent fields consult this
        // flag. Pure-plain structs never read or write it (the walker
        // emits static `,"key":` literals via `concat!`), so the
        // `#[allow(unused)]` suppresses a "let assigned never read"
        // warning for that common case.
        #[allow(unused_assignments, unused_mut, unused_variables)]
        let mut __first: bool = true;
        $crate::__to_json_named_walk!(
            self_ref: $self,
            sink: $w,
            first: __first,
            emit: { },
            cur_rename: (),
            cur_skip: (),
            cur_skip_if_none: (),
            cur_name: (),
            ftokens: [],
            static_first: (yes),
            input: ($($body)*)
        );
        $w.write_byte(b'}')?;
        ::core::result::Result::Ok(())
    }};
}

// ============================================================================
// The tt-muncher for named-struct fields.
//
// Mirrors `__from_json_walk`'s state machine but with simpler
// per-field state. The four named buckets:
//
//   - `emit`: append-only stream of per-field emit blocks.
//   - `cur_rename`, `cur_skip`, `cur_skip_if_none`: per-field
//     attribute state. Cleared after each commit.
//   - `cur_name`, `ftokens`: per-field name + type accumulator. Type
//     tokens are not used to emit anything (the impl just calls
//     `ToJson::write_json` on the value, which is type-erased through
//     trait dispatch), but we still consume them to advance the input
//     correctly.
// ============================================================================

#[doc(hidden)]
#[macro_export]
macro_rules! __to_json_named_walk {
    // ---------- Phase 1: terminal — input exhausted, no in-flight field. ----------
    (
        self_ref: $self:ident,
        sink: $w:ident,
        first: $first:ident,
        emit: { $($emit:tt)* },
        cur_rename: (),
        cur_skip: (),
        cur_skip_if_none: (),
        cur_name: (),
        ftokens: [],
        static_first: ($($_sf:tt)?),
        input: ()
    ) => {
        $($emit)*
    };

    // ---------- Phase 1b-skip: terminal commit for #[bourne(skip)]. ----------
    //
    // Skipped fields contribute nothing to `emit`. Type tokens are
    // discarded. `static_first` is preserved unchanged — a skipped
    // field neither emits punctuation nor changes the leading-comma
    // posture of the next field.
    (
        self_ref: $self:ident,
        sink: $w:ident,
        first: $first:ident,
        emit: { $($emit:tt)* },
        cur_rename: ($($_rn:tt)?),
        cur_skip: (skip),
        cur_skip_if_none: ($($_si:tt)?),
        cur_name: ($fname:ident),
        ftokens: [ $($_ft:tt)+ ],
        input: ()
    ) => {
        $crate::__to_json_named_walk!(
            self_ref: $self,
            sink: $w,
            first: $first,
            emit: { $($emit)* },
            cur_rename: (),
            cur_skip: (),
            cur_skip_if_none: (),
            cur_name: (),
            ftokens: [],
            static_first: (no),  // doesn't matter — input exhausted
            input: ()
        )
    };

    // ---------- Phase 1c: terminal — last field, skip_if_none variant. ----------
    //
    // skip_if_none always emits a runtime conditional regardless of
    // `static_first`. Two flavors:
    //   - `static_first: (yes)`: this is the *only* field, so
    //     unconditionally elide the leading comma (the `if !$first`
    //     branch is dead — `__first` is still `true`).
    //   - `static_first: (no | maybe)`: prior fields may or may not
    //     have committed. If `(no)`, a prior plain field always
    //     committed and we always need the comma. If `(maybe)`, fall
    //     back to the runtime flag.
    (
        self_ref: $self:ident,
        sink: $w:ident,
        first: $first:ident,
        emit: { $($emit:tt)* },
        cur_rename: ($($rename:tt)?),
        cur_skip: (),
        cur_skip_if_none: (yes),
        cur_name: ($fname:ident),
        ftokens: [ $($_ft:tt)+ ],
        input: ()
    ) => {
        $crate::__to_json_named_walk!(
            self_ref: $self,
            sink: $w,
            first: $first,
            emit: {
                $($emit)*
                if let ::core::option::Option::Some(ref __v) = $self.$fname {
                    if !$first { $w.write_byte(b',')?; }
                    $w.write_escaped_str(
                        $crate::__to_json_field_key!($fname, ($($rename)?))
                    )?;
                    $w.write_byte(b':')?;
                    $crate::ToJson::write_json(__v, $w)?;
                    $first = false;
                }
            },
            cur_rename: (),
            cur_skip: (),
            cur_skip_if_none: (),
            cur_name: (),
            ftokens: [],
            static_first: (no),
            input: ()
        )
    };

    // ---------- Phase 1d: terminal — last field, plain, no rename, static-known. ----------
    //
    // Fast path: no rename means the key is `stringify!($fname)`,
    // which is guaranteed to be a Rust ident — no escape-needing
    // bytes. Fuse `,"key":` (or `"key":` if first) into a single
    // `concat!()` literal and emit via `write_str_raw` (one
    // `String::push_str`).
    (
        self_ref: $self:ident,
        sink: $w:ident,
        first: $first:ident,
        emit: { $($emit:tt)* },
        cur_rename: (),
        cur_skip: (),
        cur_skip_if_none: (),
        cur_name: ($fname:ident),
        ftokens: [ $($_ft:tt)+ ],
        static_first: (yes),
        input: ()
    ) => {
        $crate::__to_json_named_walk!(
            self_ref: $self,
            sink: $w,
            first: $first,
            emit: {
                $($emit)*
                $w.write_str_raw(
                    ::core::concat!("\"", ::core::stringify!($fname), "\":")
                )?;
                $crate::ToJson::write_json(&$self.$fname, $w)?;
            },
            cur_rename: (),
            cur_skip: (),
            cur_skip_if_none: (),
            cur_name: (),
            ftokens: [],
            static_first: (no),
            input: ()
        )
    };
    (
        self_ref: $self:ident,
        sink: $w:ident,
        first: $first:ident,
        emit: { $($emit:tt)* },
        cur_rename: (),
        cur_skip: (),
        cur_skip_if_none: (),
        cur_name: ($fname:ident),
        ftokens: [ $($_ft:tt)+ ],
        static_first: (no),
        input: ()
    ) => {
        $crate::__to_json_named_walk!(
            self_ref: $self,
            sink: $w,
            first: $first,
            emit: {
                $($emit)*
                $w.write_str_raw(
                    ::core::concat!(",\"", ::core::stringify!($fname), "\":")
                )?;
                $crate::ToJson::write_json(&$self.$fname, $w)?;
            },
            cur_rename: (),
            cur_skip: (),
            cur_skip_if_none: (),
            cur_name: (),
            ftokens: [],
            static_first: (no),
            input: ()
        )
    };

    // ---------- Phase 1d: terminal — last field, plain, dynamic comma. ----------
    //
    // Slow path. Either:
    //   - the field is renamed (we keep `write_escaped_str` because
    //     the rename literal is user-controlled and may legitimately
    //     contain bytes that need escaping), or
    //   - a prior `skip_if_none` made comma placement runtime-
    //     dependent (`static_first: (maybe)`).
    (
        self_ref: $self:ident,
        sink: $w:ident,
        first: $first:ident,
        emit: { $($emit:tt)* },
        cur_rename: ($($rename:tt)?),
        cur_skip: (),
        cur_skip_if_none: (),
        cur_name: ($fname:ident),
        ftokens: [ $($_ft:tt)+ ],
        static_first: ($($_sf:tt)?),
        input: ()
    ) => {
        $crate::__to_json_named_walk!(
            self_ref: $self,
            sink: $w,
            first: $first,
            emit: {
                $($emit)*
                if !$first { $w.write_byte(b',')?; }
                $w.write_escaped_str(
                    $crate::__to_json_field_key!($fname, ($($rename)?))
                )?;
                $w.write_byte(b':')?;
                $crate::ToJson::write_json(&$self.$fname, $w)?;
                $first = false;
            },
            cur_rename: (),
            cur_skip: (),
            cur_skip_if_none: (),
            cur_name: (),
            ftokens: [],
            static_first: (maybe),
            input: ()
        )
    };

    // ---------- bourne(rename = "x") on the next field. ----------
    (
        self_ref: $self:ident,
        sink: $w:ident,
        first: $first:ident,
        emit: $emit:tt,
        cur_rename: (),
        cur_skip: ($($skip:tt)?),
        cur_skip_if_none: ($($sin:tt)?),
        cur_name: (),
        ftokens: [],
        static_first: ($($sf:tt)?),
        input: ( #[bourne(rename = $renamed:literal)] $($rest:tt)* )
    ) => {
        $crate::__to_json_named_walk!(
            self_ref: $self,
            sink: $w,
            first: $first,
            emit: $emit,
            cur_rename: ($renamed),
            cur_skip: ($($skip)?),
            cur_skip_if_none: ($($sin)?),
            cur_name: (),
            ftokens: [],
            static_first: ($($sf)?),
            input: ($($rest)*)
        )
    };

    // ---------- bourne(skip) on the next field. ----------
    (
        self_ref: $self:ident,
        sink: $w:ident,
        first: $first:ident,
        emit: $emit:tt,
        cur_rename: ($($rename:tt)?),
        cur_skip: (),
        cur_skip_if_none: ($($sin:tt)?),
        cur_name: (),
        ftokens: [],
        static_first: ($($sf:tt)?),
        input: ( #[bourne(skip)] $($rest:tt)* )
    ) => {
        $crate::__to_json_named_walk!(
            self_ref: $self,
            sink: $w,
            first: $first,
            emit: $emit,
            cur_rename: ($($rename)?),
            cur_skip: (skip),
            cur_skip_if_none: ($($sin)?),
            cur_name: (),
            ftokens: [],
            static_first: ($($sf)?),
            input: ($($rest)*)
        )
    };

    // ---------- bourne(skip_if_none) on the next field. ----------
    (
        self_ref: $self:ident,
        sink: $w:ident,
        first: $first:ident,
        emit: $emit:tt,
        cur_rename: ($($rename:tt)?),
        cur_skip: ($($skip:tt)?),
        cur_skip_if_none: (),
        cur_name: (),
        ftokens: [],
        static_first: ($($sf:tt)?),
        input: ( #[bourne(skip_if_none)] $($rest:tt)* )
    ) => {
        $crate::__to_json_named_walk!(
            self_ref: $self,
            sink: $w,
            first: $first,
            emit: $emit,
            cur_rename: ($($rename)?),
            cur_skip: ($($skip)?),
            cur_skip_if_none: (yes),
            cur_name: (),
            ftokens: [],
            static_first: ($($sf)?),
            input: ($($rest)*)
        )
    };

    // ---------- bourne(default) on the next field — no-op for ser. ----------
    (
        self_ref: $self:ident,
        sink: $w:ident,
        first: $first:ident,
        emit: $emit:tt,
        cur_rename: ($($rename:tt)?),
        cur_skip: ($($skip:tt)?),
        cur_skip_if_none: ($($sin:tt)?),
        cur_name: (),
        ftokens: [],
        static_first: ($($sf:tt)?),
        input: ( #[bourne(default)] $($rest:tt)* )
    ) => {
        $crate::__to_json_named_walk!(
            self_ref: $self,
            sink: $w,
            first: $first,
            emit: $emit,
            cur_rename: ($($rename)?),
            cur_skip: ($($skip)?),
            cur_skip_if_none: ($($sin)?),
            cur_name: (),
            ftokens: [],
            static_first: ($($sf)?),
            input: ($($rest)*)
        )
    };

    // ---------- compound: rename + default ----------
    (
        self_ref: $self:ident,
        sink: $w:ident,
        first: $first:ident,
        emit: $emit:tt,
        cur_rename: (),
        cur_skip: ($($skip:tt)?),
        cur_skip_if_none: ($($sin:tt)?),
        cur_name: (),
        ftokens: [],
        static_first: ($($sf:tt)?),
        input: ( #[bourne(rename = $renamed:literal, default)] $($rest:tt)* )
    ) => {
        $crate::__to_json_named_walk!(
            self_ref: $self,
            sink: $w,
            first: $first,
            emit: $emit,
            cur_rename: ($renamed),
            cur_skip: ($($skip)?),
            cur_skip_if_none: ($($sin)?),
            cur_name: (),
            ftokens: [],
            static_first: ($($sf)?),
            input: ($($rest)*)
        )
    };

    // ---------- Phase 2 entry: read field name. ----------
    (
        self_ref: $self:ident,
        sink: $w:ident,
        first: $first:ident,
        emit: $emit:tt,
        cur_rename: ($($rename:tt)?),
        cur_skip: ($($skip:tt)?),
        cur_skip_if_none: ($($sin:tt)?),
        cur_name: (),
        ftokens: [],
        static_first: ($($sf:tt)?),
        input: ( $_fvis:vis $fname:ident : $($rest:tt)* )
    ) => {
        $crate::__to_json_named_walk!(
            self_ref: $self,
            sink: $w,
            first: $first,
            emit: $emit,
            cur_rename: ($($rename)?),
            cur_skip: ($($skip)?),
            cur_skip_if_none: ($($sin)?),
            cur_name: ($fname),
            ftokens: [],
            static_first: ($($sf)?),
            input: ($($rest)*)
        )
    };

    // ---------- Phase 2: comma commits the in-flight field. ----------
    //
    // skip variant: drop the field on the floor. `static_first` is
    // unchanged — a skipped field neither emits nor changes the
    // leading-comma posture of the next field.
    (
        self_ref: $self:ident,
        sink: $w:ident,
        first: $first:ident,
        emit: $emit:tt,
        cur_rename: ($($_rn:tt)?),
        cur_skip: (skip),
        cur_skip_if_none: ($($_si:tt)?),
        cur_name: ($fname:ident),
        ftokens: [ $($_ft:tt)+ ],
        static_first: ($($sf:tt)?),
        input: ( , $($rest:tt)* )
    ) => {
        $crate::__to_json_named_walk!(
            self_ref: $self,
            sink: $w,
            first: $first,
            emit: $emit,
            cur_rename: (),
            cur_skip: (),
            cur_skip_if_none: (),
            cur_name: (),
            ftokens: [],
            static_first: ($($sf)?),
            input: ($($rest)*)
        )
    };

    // skip_if_none variant: emit the conditional block. After the
    // block, comma posture is runtime-dependent (the field may or may
    // not have committed), so transition `static_first` to (maybe).
    //
    // Two sub-arms specialize the leading-comma computation by current
    // `static_first`: (yes) means we're definitely the first emit and
    // the comma never fires; (no | maybe) keeps the runtime branch.
    (
        self_ref: $self:ident,
        sink: $w:ident,
        first: $first:ident,
        emit: { $($emit:tt)* },
        cur_rename: ($($rename:tt)?),
        cur_skip: (),
        cur_skip_if_none: (yes),
        cur_name: ($fname:ident),
        ftokens: [ $($_ft:tt)+ ],
        static_first: (yes),
        input: ( , $($rest:tt)* )
    ) => {
        $crate::__to_json_named_walk!(
            self_ref: $self,
            sink: $w,
            first: $first,
            emit: {
                $($emit)*
                if let ::core::option::Option::Some(ref __v) = $self.$fname {
                    $w.write_escaped_str(
                        $crate::__to_json_field_key!($fname, ($($rename)?))
                    )?;
                    $w.write_byte(b':')?;
                    $crate::ToJson::write_json(__v, $w)?;
                    $first = false;
                }
            },
            cur_rename: (),
            cur_skip: (),
            cur_skip_if_none: (),
            cur_name: (),
            ftokens: [],
            static_first: (maybe),
            input: ($($rest)*)
        )
    };
    (
        self_ref: $self:ident,
        sink: $w:ident,
        first: $first:ident,
        emit: { $($emit:tt)* },
        cur_rename: ($($rename:tt)?),
        cur_skip: (),
        cur_skip_if_none: (yes),
        cur_name: ($fname:ident),
        ftokens: [ $($_ft:tt)+ ],
        static_first: ($($_sf:tt)?),
        input: ( , $($rest:tt)* )
    ) => {
        $crate::__to_json_named_walk!(
            self_ref: $self,
            sink: $w,
            first: $first,
            emit: {
                $($emit)*
                if let ::core::option::Option::Some(ref __v) = $self.$fname {
                    if !$first { $w.write_byte(b',')?; }
                    $w.write_escaped_str(
                        $crate::__to_json_field_key!($fname, ($($rename)?))
                    )?;
                    $w.write_byte(b':')?;
                    $crate::ToJson::write_json(__v, $w)?;
                    $first = false;
                }
            },
            cur_rename: (),
            cur_skip: (),
            cur_skip_if_none: (),
            cur_name: (),
            ftokens: [],
            static_first: (maybe),
            input: ($($rest)*)
        )
    };

    // plain variant, no rename, static-first: yes.
    //
    // Fast path. `concat!("\"", stringify!($fname), "\":")` collapses
    // to a single &'static str, and the value is the only field
    // committed so far so no leading comma. Single `write_str_raw`
    // (one `String::push_str`) replaces the old 4 sink calls.
    //
    // `$first = false;` keeps the runtime flag in sync so a later
    // skip_if_none or renamed field that falls back to the dynamic
    // arm sees the correct posture. The store is hoisted out of any
    // loop in the caller and disappears under register allocation.
    (
        self_ref: $self:ident,
        sink: $w:ident,
        first: $first:ident,
        emit: { $($emit:tt)* },
        cur_rename: (),
        cur_skip: (),
        cur_skip_if_none: (),
        cur_name: ($fname:ident),
        ftokens: [ $($_ft:tt)+ ],
        static_first: (yes),
        input: ( , $($rest:tt)* )
    ) => {
        $crate::__to_json_named_walk!(
            self_ref: $self,
            sink: $w,
            first: $first,
            emit: {
                $($emit)*
                $w.write_str_raw(
                    ::core::concat!("\"", ::core::stringify!($fname), "\":")
                )?;
                $crate::ToJson::write_json(&$self.$fname, $w)?;
                $first = false;
            },
            cur_rename: (),
            cur_skip: (),
            cur_skip_if_none: (),
            cur_name: (),
            ftokens: [],
            static_first: (no),
            input: ($($rest)*)
        )
    };

    // plain variant, no rename, static-first: no.
    //
    // Fast path. The leading comma is also static, so fold it into
    // the same literal: `,"key":`.
    (
        self_ref: $self:ident,
        sink: $w:ident,
        first: $first:ident,
        emit: { $($emit:tt)* },
        cur_rename: (),
        cur_skip: (),
        cur_skip_if_none: (),
        cur_name: ($fname:ident),
        ftokens: [ $($_ft:tt)+ ],
        static_first: (no),
        input: ( , $($rest:tt)* )
    ) => {
        $crate::__to_json_named_walk!(
            self_ref: $self,
            sink: $w,
            first: $first,
            emit: {
                $($emit)*
                $w.write_str_raw(
                    ::core::concat!(",\"", ::core::stringify!($fname), "\":")
                )?;
                $crate::ToJson::write_json(&$self.$fname, $w)?;
            },
            cur_rename: (),
            cur_skip: (),
            cur_skip_if_none: (),
            cur_name: (),
            ftokens: [],
            static_first: (no),
            input: ($($rest)*)
        )
    };

    // plain variant, dynamic comma fallback (rename set, OR a prior
    // `skip_if_none` made comma posture runtime-dependent).
    //
    // - Rename: `$renamed` is user-controlled and may contain bytes
    //   that need escaping; route through `write_escaped_str`.
    // - `static_first: (maybe)`: consult the runtime `__first` flag.
    (
        self_ref: $self:ident,
        sink: $w:ident,
        first: $first:ident,
        emit: { $($emit:tt)* },
        cur_rename: ($($rename:tt)?),
        cur_skip: (),
        cur_skip_if_none: (),
        cur_name: ($fname:ident),
        ftokens: [ $($_ft:tt)+ ],
        static_first: ($($_sf:tt)?),
        input: ( , $($rest:tt)* )
    ) => {
        $crate::__to_json_named_walk!(
            self_ref: $self,
            sink: $w,
            first: $first,
            emit: {
                $($emit)*
                if !$first { $w.write_byte(b',')?; }
                $w.write_escaped_str(
                    $crate::__to_json_field_key!($fname, ($($rename)?))
                )?;
                $w.write_byte(b':')?;
                $crate::ToJson::write_json(&$self.$fname, $w)?;
                $first = false;
            },
            cur_rename: (),
            cur_skip: (),
            cur_skip_if_none: (),
            cur_name: (),
            ftokens: [],
            static_first: (maybe),
            input: ($($rest)*)
        )
    };

    // ---------- Phase 2: type-token accumulator. ----------
    //
    // Walk the field's type one token at a time. Each token gets
    // pushed onto `ftokens`. We never inspect ftokens — type info is
    // carried by `&self.fname` at runtime — but we still need to
    // consume each token to keep the input cursor advancing.
    (
        self_ref: $self:ident,
        sink: $w:ident,
        first: $first:ident,
        emit: $emit:tt,
        cur_rename: ($($rename:tt)?),
        cur_skip: ($($skip:tt)?),
        cur_skip_if_none: ($($sin:tt)?),
        cur_name: ($fname:ident),
        ftokens: [ $($ftokens:tt)* ],
        static_first: ($($sf:tt)?),
        input: ( $tok:tt $($rest:tt)* )
    ) => {
        $crate::__to_json_named_walk!(
            self_ref: $self,
            sink: $w,
            first: $first,
            emit: $emit,
            cur_rename: ($($rename)?),
            cur_skip: ($($skip)?),
            cur_skip_if_none: ($($sin)?),
            cur_name: ($fname),
            ftokens: [ $($ftokens)* $tok ],
            static_first: ($($sf)?),
            input: ($($rest)*)
        )
    };
}

// ============================================================================
// Field-key resolver: rename overrides the ident's stringified form.
// Mirrors `__from_json_field_key`.
// ============================================================================

#[doc(hidden)]
#[macro_export]
macro_rules! __to_json_field_key {
    ($fname:ident, ($renamed:literal)) => { $renamed };
    ($fname:ident, ()) => { ::core::stringify!($fname) };
}

// ============================================================================
// `json!` — combined macro that emits the type definition once and
// generates both `FromJson` and `ToJson` impls.
//
// Without this, users who need both traits must choose between
// duplicating the struct definition or writing one impl by hand,
// because `from_json!` and `to_json!` each emit the type definition
// themselves.
//
// Every arm emits the type via the existing `__from_json_emit_struct_def!`
// / `__from_json_emit_enum_def!`, then splices both impl blocks.
// ============================================================================

/// Emit a struct or enum together with both its [`FromJson`](crate::FromJson)
/// and [`ToJson`](crate::ToJson) impls.
///
/// This is the combined form of [`from_json!`] and [`to_json!`]. Use it
/// when a type needs to both parse and serialize — it emits the type
/// definition once, avoiding the duplicate-definition error you'd get
/// from invoking both single-trait macros on the same type.
///
/// ```
/// use bourne::{json, parse_str, to_string};
///
/// json! {
///     #[derive(Debug, PartialEq)]
///     struct Point { x: i32, y: i32 }
/// }
///
/// let p: Point = parse_str(r#"{"x":1,"y":2}"#).unwrap();
/// assert_eq!(to_string(&p).unwrap(), r#"{"x":1,"y":2}"#);
/// ```
#[macro_export]
macro_rules! json {
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

        impl $crate::ToJson for $name {
            fn write_json<__W: $crate::JsonWrite + ?::core::marker::Sized>(
                &self,
                __w: &mut __W,
            ) -> ::core::result::Result<(), __W::Error> {
                $crate::__to_json_named_body!(self, __w, $($body)*)
            }
        }
    };

    // -----------------------------------------------------------------
    // Named-field struct, one lifetime.
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

        impl<$lt> $crate::ToJson for $name<$lt> {
            fn write_json<__W: $crate::JsonWrite + ?::core::marker::Sized>(
                &self,
                __w: &mut __W,
            ) -> ::core::result::Result<(), __W::Error> {
                $crate::__to_json_named_body!(self, __w, $($body)*)
            }
        }
    };

    // -----------------------------------------------------------------
    // Tuple struct, newtype, no generics.
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

        impl $crate::ToJson for $name {
            fn write_json<__W: $crate::JsonWrite + ?::core::marker::Sized>(
                &self,
                __w: &mut __W,
            ) -> ::core::result::Result<(), __W::Error> {
                $crate::ToJson::write_json(&self.0, __w)
            }
        }
    };

    // -----------------------------------------------------------------
    // Tuple struct, newtype, one lifetime.
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

        impl<$lt> $crate::ToJson for $name<$lt> {
            fn write_json<__W: $crate::JsonWrite + ?::core::marker::Sized>(
                &self,
                __w: &mut __W,
            ) -> ::core::result::Result<(), __W::Error> {
                $crate::ToJson::write_json(&self.0, __w)
            }
        }
    };

    // -----------------------------------------------------------------
    // Tuple struct, multi-field, no generics.
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

        impl $crate::ToJson for $name {
            fn write_json<__W: $crate::JsonWrite + ?::core::marker::Sized>(
                &self,
                __w: &mut __W,
            ) -> ::core::result::Result<(), __W::Error> {
                __w.write_byte(b'[')?;
                $crate::ToJson::write_json(&self.0, __w)?;
                $crate::__to_json_tuple_walk!(
                    self_ref: self,
                    sink: __w,
                    idx: 1,
                    remaining: [ $(($ftyn))+ ]
                );
                __w.write_byte(b']')?;
                ::core::result::Result::Ok(())
            }
        }
    };

    // -----------------------------------------------------------------
    // Tuple struct, multi-field, one lifetime.
    // -----------------------------------------------------------------
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

        impl<$lt> $crate::ToJson for $name<$lt> {
            fn write_json<__W: $crate::JsonWrite + ?::core::marker::Sized>(
                &self,
                __w: &mut __W,
            ) -> ::core::result::Result<(), __W::Error> {
                __w.write_byte(b'[')?;
                $crate::ToJson::write_json(&self.0, __w)?;
                $crate::__to_json_tuple_walk!(
                    self_ref: self,
                    sink: __w,
                    idx: 1,
                    remaining: [ $(($ftyn))+ ]
                );
                __w.write_byte(b']')?;
                ::core::result::Result::Ok(())
            }
        }
    };

    // -----------------------------------------------------------------
    // Untagged enum, no generics.
    // -----------------------------------------------------------------
    (
        #[bourne(untagged)]
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
                $crate::__from_json_untagged_dispatch!(__lex, $name, ($($variants)*))
            }
        }

        impl $crate::ToJson for $name {
            fn write_json<__W: $crate::JsonWrite + ?::core::marker::Sized>(
                &self,
                __w: &mut __W,
            ) -> ::core::result::Result<(), __W::Error> {
                let __this: &Self = self;
                $crate::__to_json_untagged_walk!(
                    receiver: __this,
                    sink: __w,
                    name: $name,
                    arms: { },
                    input: ($($variants)*)
                )
            }
        }
    };

    // Untagged enum, one lifetime.
    (
        #[bourne(untagged)]
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
                $crate::__from_json_untagged_dispatch!(__lex, $name, ($($variants)*))
            }
        }

        impl<$lt> $crate::ToJson for $name<$lt> {
            fn write_json<__W: $crate::JsonWrite + ?::core::marker::Sized>(
                &self,
                __w: &mut __W,
            ) -> ::core::result::Result<(), __W::Error> {
                let __this: &Self = self;
                $crate::__to_json_untagged_walk!(
                    receiver: __this,
                    sink: __w,
                    name: $name,
                    arms: { },
                    input: ($($variants)*)
                )
            }
        }
    };

    // -----------------------------------------------------------------
    // Adjacently-tagged enum, no generics.
    // -----------------------------------------------------------------
    (
        #[bourne(tag = $tag:literal, content = $content:literal)]
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
                $crate::__from_json_adjacently_tagged_dispatch!(
                    __lex, $name, $tag, $content, ($($variants)*)
                )
            }
        }

        impl $crate::ToJson for $name {
            fn write_json<__W: $crate::JsonWrite + ?::core::marker::Sized>(
                &self,
                __w: &mut __W,
            ) -> ::core::result::Result<(), __W::Error> {
                let __this: &Self = self;
                $crate::__to_json_adjacent_walk!(
                    receiver: __this,
                    sink: __w,
                    name: $name,
                    tag: $tag,
                    content: $content,
                    arms: { },
                    cur_rename: (),
                    input: ($($variants)*)
                )
            }
        }
    };

    // Adjacently-tagged enum, one lifetime.
    (
        #[bourne(tag = $tag:literal, content = $content:literal)]
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
                $crate::__from_json_adjacently_tagged_dispatch!(
                    __lex, $name, $tag, $content, ($($variants)*)
                )
            }
        }

        impl<$lt> $crate::ToJson for $name<$lt> {
            fn write_json<__W: $crate::JsonWrite + ?::core::marker::Sized>(
                &self,
                __w: &mut __W,
            ) -> ::core::result::Result<(), __W::Error> {
                let __this: &Self = self;
                $crate::__to_json_adjacent_walk!(
                    receiver: __this,
                    sink: __w,
                    name: $name,
                    tag: $tag,
                    content: $content,
                    arms: { },
                    cur_rename: (),
                    input: ($($variants)*)
                )
            }
        }
    };

    // -----------------------------------------------------------------
    // Internally-tagged enum, no generics.
    // -----------------------------------------------------------------
    (
        #[bourne(tag = $tag:literal)]
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
                $crate::__from_json_internally_tagged_dispatch!(
                    __lex, $name, $tag, ($($variants)*)
                )
            }
        }

        impl $crate::ToJson for $name {
            fn write_json<__W: $crate::JsonWrite + ?::core::marker::Sized>(
                &self,
                __w: &mut __W,
            ) -> ::core::result::Result<(), __W::Error> {
                let __this: &Self = self;
                $crate::__to_json_internal_walk!(
                    receiver: __this,
                    sink: __w,
                    name: $name,
                    tag: $tag,
                    arms: { },
                    cur_rename: (),
                    input: ($($variants)*)
                )
            }
        }
    };

    // Internally-tagged enum, one lifetime.
    (
        #[bourne(tag = $tag:literal)]
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
                $crate::__from_json_internally_tagged_dispatch!(
                    __lex, $name, $tag, ($($variants)*)
                )
            }
        }

        impl<$lt> $crate::ToJson for $name<$lt> {
            fn write_json<__W: $crate::JsonWrite + ?::core::marker::Sized>(
                &self,
                __w: &mut __W,
            ) -> ::core::result::Result<(), __W::Error> {
                let __this: &Self = self;
                $crate::__to_json_internal_walk!(
                    receiver: __this,
                    sink: __w,
                    name: $name,
                    tag: $tag,
                    arms: { },
                    cur_rename: (),
                    input: ($($variants)*)
                )
            }
        }
    };

    // -----------------------------------------------------------------
    // Externally-tagged enum, no generics.
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

        impl $crate::ToJson for $name {
            fn write_json<__W: $crate::JsonWrite + ?::core::marker::Sized>(
                &self,
                __w: &mut __W,
            ) -> ::core::result::Result<(), __W::Error> {
                let __this: &Self = self;
                $crate::__to_json_enum_walk!(
                    receiver: __this,
                    sink: __w,
                    name: $name,
                    arms: { },
                    cur_rename: (),
                    input: ($($variants)*)
                )
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

        impl<$lt> $crate::ToJson for $name<$lt> {
            fn write_json<__W: $crate::JsonWrite + ?::core::marker::Sized>(
                &self,
                __w: &mut __W,
            ) -> ::core::result::Result<(), __W::Error> {
                let __this: &Self = self;
                $crate::__to_json_enum_walk!(
                    receiver: __this,
                    sink: __w,
                    name: $name,
                    arms: { },
                    cur_rename: (),
                    input: ($($variants)*)
                )
            }
        }
    };
}

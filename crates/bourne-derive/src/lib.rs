//! `syn`-based derive macros for `json-bourne`.
//!
//! This crate is not used directly. Enable the `derive` feature on
//! `json-bourne` and use `#[derive(FromJson, ToJson)]` — the derives are
//! re-exported from the main crate so there is a single import surface.
//!
//! Generated code targets the crate runtime (`Lexer` / `JsonWrite` /
//! `FromJson` / `ToJson`) via `::json_bourne::…` paths that resolve in
//! the consumer's crate graph.
//!
//! Supported shapes: named and tuple structs, and all four enum tagging
//! modes (external, internal `#[bourne(tag)]`, adjacent `#[bourne(tag,
//! content)]`, untagged), both directions. Field attributes: `rename`,
//! `skip`, `default`, `skip_if_none`. Container attributes: `rename_all`,
//! `deny_unknown_fields`, `tag`, `content`, `untagged`.
//!
//! Module layout: `attrs` parses `#[bourne(...)]`; `field_plan` turns a
//! field plus its attributes into the one model both directions consume;
//! `naming` builds JSON keys and match heads; `acquire` picks the
//! parse fast path for a type; `generics` plans the impl headers;
//! `shape` classifies variants; and `from_json` / `to_json` hold the
//! codegen strategies.

mod acquire;
mod attrs;
mod field_plan;
mod from_json;
mod generics;
mod naming;
mod shape;
mod to_json;

use proc_macro::TokenStream;
use syn::{DeriveInput, parse_macro_input};

use from_json::FromJsonDerive;
use to_json::ToJsonDerive;

#[proc_macro_derive(FromJson, attributes(bourne))]
pub fn derive_from_json(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match FromJsonDerive::expand(&input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

#[proc_macro_derive(ToJson, attributes(bourne))]
pub fn derive_to_json(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match ToJsonDerive::expand(&input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

#[cfg(test)]
mod empty_tuple_rejection_tests {
    use super::{FromJsonDerive, ToJsonDerive};
    use syn::DeriveInput;

    fn derive(src: &str) -> syn::Result<()> {
        let input = syn::parse_str::<DeriveInput>(src).unwrap();
        FromJsonDerive::expand(&input)?;
        ToJsonDerive::expand(&input)?;
        Ok(())
    }

    #[test]
    fn empty_tuple_struct_is_rejected() {
        let err = derive("struct Empty();").unwrap_err();
        assert!(
            err.to_string()
                .contains("empty tuple structs are unsupported"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn empty_tuple_variant_is_rejected_in_every_mode() {
        for src in [
            "enum E { V() }",
            "#[bourne(untagged)] enum E { V() }",
            "#[bourne(tag = \"t\")] enum E { V() }",
            "#[bourne(tag = \"t\", content = \"c\")] enum E { V() }",
        ] {
            let err = derive(src).unwrap_err();
            assert!(
                err.to_string()
                    .contains("empty tuple variants are unsupported"),
                "{src}: unexpected error: {err}"
            );
        }
    }

    #[test]
    fn non_empty_tuples_still_derive() {
        derive("struct T(u8);").unwrap();
        derive("struct T(u8, u16);").unwrap();
        derive("enum E { V(u8, u16), U }").unwrap();
    }
}

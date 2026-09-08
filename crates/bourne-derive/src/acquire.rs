//! Value-acquisition codegen: the integer and `&str` fast paths that
//! bypass the generic `FromJson::from_lex` dispatch. Bench fairness
//! depends on these matching the hand-written equivalents exactly.

use proc_macro2::TokenStream;
use quote::quote;
use syn::Type;

use crate::naming::Naming;

pub(crate) struct Acquire;

impl Acquire {
    pub(crate) fn expr(ty: &Type) -> TokenStream {
        // `&str` and `&'a str` for any lifetime: match on type
        // structure, not the stringified type, so a lifetime name other
        // than `'input` still takes the direct string path (audit 4.3.4).
        if let Type::Reference(tr) = ty {
            if tr.mutability.is_none() && Naming::is_bare_str(&tr.elem) {
                return quote! { __lex.parse_str_value()? };
            }
        }
        let ty_str = quote!(#ty).to_string().replace(' ', "");
        match ty_str.as_str() {
            "i64" => quote! { __lex.parse_i64_value()? },
            "u64" => quote! { __lex.parse_u64_value()? },
            "f64" => quote! { __lex.parse_f64_value()? },
            "f32" => quote! { __lex.parse_f64_value()? as f32 },
            "i8" => Self::int_narrow("i8"),
            "i16" => Self::int_narrow("i16"),
            "i32" => Self::int_narrow("i32"),
            "isize" => Self::int_narrow("isize"),
            "u8" => Self::uint_narrow("u8"),
            "u16" => Self::uint_narrow("u16"),
            "u32" => Self::uint_narrow("u32"),
            "usize" => Self::uint_narrow("usize"),
            "bool" => quote! {
                match __lex.read_value()? {
                    ::json_bourne::Event::Bool(b) => b,
                    _ => return ::core::result::Result::Err(::json_bourne::Error::new(
                        ::json_bourne::ErrorKind::ExpectedBool,
                        __lex.position(),
                    )),
                }
            },
            _ => quote! {
                <#ty as ::json_bourne::FromJson<'_>>::from_lex(__lex)?
            },
        }
    }

    fn int_narrow(t: &str) -> TokenStream {
        let t: TokenStream = t.parse().unwrap();
        quote! {
            <#t>::try_from(__lex.parse_i64_value()?).map_err(|_| {
                ::json_bourne::Error::new(
                    ::json_bourne::ErrorKind::NumberOutOfRange,
                    __lex.position(),
                )
            })?
        }
    }

    fn uint_narrow(t: &str) -> TokenStream {
        let t: TokenStream = t.parse().unwrap();
        quote! {
            <#t>::try_from(__lex.parse_u64_value()?).map_err(|_| {
                ::json_bourne::Error::new(
                    ::json_bourne::ErrorKind::NumberOutOfRange,
                    __lex.position(),
                )
            })?
        }
    }
}

//! JSON key construction and the small type predicates the codegen
//! branches on.

use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::{Ident, Type};

pub(crate) struct Naming;

impl Naming {
    /// Whether `ty` is exactly `str` — a last path segment named `str`
    /// with no further qualification.
    pub(crate) fn is_bare_str(ty: &Type) -> bool {
        matches!(ty, Type::Path(tp) if tp.qself.is_none() && tp.path.get_ident().is_some_and(|i| i == "str"))
    }

    /// The JSON key expression for a field or variant: explicit rename
    /// wins, else container `rename_all`, else the verbatim name.
    pub(crate) fn key_expr(
        name: &Ident,
        rename: &Option<String>,
        rename_all: &Option<String>,
    ) -> TokenStream {
        if let Some(r) = rename {
            quote! { #r }
        } else if let Some(case) = rename_all {
            let name_str = name.to_string();
            quote! {{
                const __BOURNE_KEY: &str =
                    ::json_bourne::__Casing::rename(#name_str, #case).as_str();
                __BOURNE_KEY
            }}
        } else {
            let name_str = name.to_string();
            quote! { #name_str }
        }
    }

    /// Match-arm head for a JSON key, literal when possible so rustc
    /// builds a length-switched byte compare instead of evaluating
    /// guards one by one (audit 4.3.1).
    ///
    /// Plain and explicitly renamed keys are string-literal patterns
    /// directly. `rename_all` keys are not known textually at expansion
    /// time, but `Casing::convert` is a `const fn`: the renamed key is
    /// hoisted into a named `const` (unique per owner so several cased
    /// keys can share one match) and the const name is the pattern.
    /// Returns the const declaration to emit beside the match — empty
    /// for literal keys — and the arm head. Anything that cannot be
    /// constant falls back to a guard arm, which stays correct.
    pub(crate) fn key_arm_head(
        key: &TokenStream,
        owner: &Ident,
        binder: &str,
    ) -> (TokenStream, TokenStream) {
        let text = key.to_string();
        let is_literal = syn::parse_str::<syn::Expr>(&text)
            .ok()
            .is_some_and(|e| matches!(e, syn::Expr::Lit(_)));
        if is_literal {
            return (quote!(), quote! { #key });
        }

        let cased = syn::parse_str::<syn::Block>(&text).ok().and_then(|b| {
            let Some(syn::Stmt::Item(syn::Item::Const(item))) = b.stmts.into_iter().next() else {
                return None;
            };
            let syn::ItemConst { expr, .. } = &item;
            let cname = Ident::new(
                &format!("__BOURNE_KEY_{}", owner.to_string().to_ascii_uppercase()),
                Span::call_site(),
            );
            Some((quote! { const #cname: &str = #expr; }, cname))
        });
        if let Some((decl, cname)) = cased {
            return (decl, quote! { #cname });
        }

        let b = Ident::new(binder, Span::call_site());
        (quote!(), quote! { #b if #b == #key })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acquire::Acquire;

    fn block(text: &str) -> TokenStream {
        text.parse().unwrap()
    }

    #[test]
    fn string_literal_yields_pattern_no_decl() {
        let key = quote! { "userId" };
        let (decl, head) =
            Naming::key_arm_head(&key, &Ident::new("user_id", Span::call_site()), "__b");
        assert!(decl.is_empty());
        assert_eq!(head.to_string(), "\"userId\"");
    }

    #[test]
    fn rename_all_const_is_hoisted_with_owner_name() {
        let key = block(
            "{ const __BOURNE_KEY: &str = ::json_bourne::__Casing::rename(\"user_id\", \"camelCase\").as_str(); __BOURNE_KEY }",
        );
        let (decl, head) =
            Naming::key_arm_head(&key, &Ident::new("user_id", Span::call_site()), "__b");
        assert_eq!(
            decl.to_string(),
            "const __BOURNE_KEY_USER_ID : & str = :: json_bourne :: __Casing :: rename (\"user_id\" , \"camelCase\") . as_str () ;"
        );
        assert_eq!(head.to_string(), "__BOURNE_KEY_USER_ID");
    }

    #[test]
    fn different_owners_get_different_consts() {
        let key = block("{ const __BOURNE_KEY: &str = \"x\"; __BOURNE_KEY }");
        let (_, h1) = Naming::key_arm_head(&key, &Ident::new("ab", Span::call_site()), "__b");
        let (_, h2) = Naming::key_arm_head(&key, &Ident::new("cd", Span::call_site()), "__b");
        assert_ne!(h1.to_string(), h2.to_string());
        assert_eq!(h1.to_string(), "__BOURNE_KEY_AB");
        assert_eq!(h2.to_string(), "__BOURNE_KEY_CD");
    }

    #[test]
    fn unexpected_shape_falls_back_to_guard() {
        let key = quote! { make_key("x") };
        let (decl, head) = Naming::key_arm_head(&key, &Ident::new("f", Span::call_site()), "__b");
        assert!(decl.is_empty());
        assert_eq!(head.to_string(), "__b if __b == make_key (\"x\")");
    }

    fn acquire(ty: &str) -> String {
        let ty: syn::Type = syn::parse_str(ty).unwrap();
        Acquire::expr(&ty).to_string()
    }

    #[test]
    fn borrowed_str_fast_path_accepts_any_lifetime() {
        for ty in ["&str", "&'input str", "&'a str", "&'listener str"] {
            assert_eq!(
                acquire(ty),
                "__lex . parse_str_value () ?",
                "fast path for {ty}"
            );
        }
    }

    #[test]
    fn non_str_references_fall_through_to_generic() {
        assert_ne!(acquire("&'a String"), "__lex . parse_str_value () ?");
        assert_ne!(acquire("&'a mut str"), "__lex . parse_str_value () ?");
        assert_ne!(acquire("&'a [u8]"), "__lex . parse_str_value () ?");
    }
}

//! Generics planning shared by both derives.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{DeriveInput, GenericParam, Lifetime, LifetimeParam};

/// Split generics into the pieces both impls need, synthesizing the
/// `'input` lifetime the runtime trait is parameterized over. If the type
/// already carries a lifetime, reuse it (bourne's convention); otherwise
/// introduce a fresh `'input`.
pub(crate) struct GenericsPlan {
    pub(crate) input_lt: Lifetime,
    /// Generics for the `impl<...>` — includes a fresh `'input` iff needed.
    pub(crate) impl_generics: TokenStream,
    /// The `TypeName<...>` reference (original params only).
    pub(crate) ty_generics: TokenStream,
    pub(crate) where_clause: TokenStream,
}

impl GenericsPlan {
    pub(crate) fn build(input: &DeriveInput) -> Self {
        let generics = &input.generics;
        let name = &input.ident;

        let existing_lt = generics.params.iter().find_map(|p| match p {
            GenericParam::Lifetime(lt) => Some(lt.lifetime.clone()),
            _ => None,
        });

        let (input_lt, needs_fresh) = match existing_lt {
            Some(lt) => (lt, false),
            None => (Lifetime::new("'input", name.span()), true),
        };

        let mut impl_params: Vec<TokenStream> = Vec::new();
        if needs_fresh {
            let lp = LifetimeParam::new(input_lt.clone());
            impl_params.push(quote!(#lp));
        }
        for p in &generics.params {
            impl_params.push(quote!(#p));
        }

        let ty_params: Vec<TokenStream> = generics
            .params
            .iter()
            .map(|p| match p {
                GenericParam::Lifetime(lt) => {
                    let l = &lt.lifetime;
                    quote!(#l)
                }
                GenericParam::Type(t) => {
                    let id = &t.ident;
                    quote!(#id)
                }
                GenericParam::Const(c) => {
                    let id = &c.ident;
                    quote!(#id)
                }
            })
            .collect();

        let ty_generics = if ty_params.is_empty() {
            quote!()
        } else {
            quote!(< #(#ty_params),* >)
        };

        // Every type param must be FromJson<'input> on the parse side.
        // ToJson doesn't need the bound and builds its own.
        let type_param_bounds: Vec<TokenStream> = generics
            .params
            .iter()
            .filter_map(|p| match p {
                GenericParam::Type(t) => {
                    let id = &t.ident;
                    Some(quote!(#id: ::json_bourne::FromJson<#input_lt>))
                }
                _ => None,
            })
            .collect();

        let where_clause = if type_param_bounds.is_empty() {
            quote!()
        } else {
            quote!(where #(#type_param_bounds),*)
        };

        Self {
            input_lt,
            impl_generics: quote!(< #(#impl_params),* >),
            ty_generics,
            where_clause,
        }
    }
}

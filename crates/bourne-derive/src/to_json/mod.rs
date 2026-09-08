//! `ToJson` codegen.

pub(crate) mod enums;
pub(crate) mod object;

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, GenericParam, Ident, spanned::Spanned};

use crate::attrs::ContainerAttrs;
use crate::field_plan::FieldPlan;
use object::ObjectWriter;

pub(crate) struct ToJsonDerive;

impl ToJsonDerive {
    pub(crate) fn expand(input: &DeriveInput) -> syn::Result<TokenStream> {
        let name = &input.ident;
        let container = ContainerAttrs::parse(&input.attrs)?;

        // ToJson has no `'input` bound; use the type's own generics
        // verbatim, plus a `T: ToJson` bound per type param.
        let generics = &input.generics;
        let (impl_g, ty_g, _where_g) = generics.split_for_impl();
        let type_param_bounds: Vec<TokenStream> = generics
            .params
            .iter()
            .filter_map(|p| match p {
                GenericParam::Type(t) => {
                    let id = &t.ident;
                    Some(quote!(#id: ::json_bourne::ToJson))
                }
                _ => None,
            })
            .collect();
        let where_clause = if type_param_bounds.is_empty() {
            quote!()
        } else {
            quote!(where #(#type_param_bounds),*)
        };

        let body = match &input.data {
            Data::Struct(s) => match &s.fields {
                Fields::Named(named) => Self::named(&named.named, &container)?,
                Fields::Unnamed(unnamed) if !unnamed.unnamed.is_empty() => {
                    Self::tuple(unnamed.unnamed.len())
                }
                Fields::Unnamed(_) => {
                    return Err(syn::Error::new(
                        input.span(),
                        "empty tuple structs are unsupported: to_json emits `[]` but \
                         the parser rejects an empty array for a zero-field tuple",
                    ));
                }
                Fields::Unit => {
                    return Err(syn::Error::new(input.span(), "unit structs unsupported"));
                }
            },
            Data::Enum(e) => enums::EnumWriter::expand(name, &e.variants, &container)?,
            Data::Union(_) => {
                return Err(syn::Error::new(input.span(), "unions are unsupported"));
            }
        };

        Ok(quote! {
            impl #impl_g ::json_bourne::ToJson for #name #ty_g #where_clause {
                fn write_json<__W: ::json_bourne::JsonWrite + ?::core::marker::Sized>(
                    &self,
                    __w: &mut __W,
                ) -> ::core::result::Result<(), __W::Error> {
                    #body
                }
            }
        })
    }

    fn named(
        fields: &syn::punctuated::Punctuated<syn::Field, syn::Token![,]>,
        container: &ContainerAttrs,
    ) -> syn::Result<TokenStream> {
        let plans = FieldPlan::for_struct(fields, &container.rename_all)?;
        let members = ObjectWriter::write(&plans, false);
        Ok(quote! {
            __w.begin_object()?;
            #members
            __w.end_object()?;
            ::core::result::Result::Ok(())
        })
    }

    fn tuple(n: usize) -> TokenStream {
        if n == 1 {
            return quote! {
                ::json_bourne::ToJson::write_json(&self.0, __w)
            };
        }
        let mut stmts = Vec::new();
        stmts.push(quote! { __w.begin_array()?; });
        for i in 0..n {
            let idx = syn::Index::from(i);
            if i > 0 {
                stmts.push(quote! { __w.separator()?; });
            }
            stmts.push(quote! { ::json_bourne::ToJson::write_json(&self.#idx, __w)?; });
        }
        stmts.push(quote! { __w.end_array()?; });
        stmts.push(quote! { ::core::result::Result::Ok(()) });
        quote! { #(#stmts)* }
    }

    /// Write bindings `__f0..` as a JSON array — a tuple variant's payload.
    pub(crate) fn tuple_payload(binds: &[Ident]) -> TokenStream {
        let mut stmts = Vec::new();
        stmts.push(quote! { __w.begin_array()?; });
        for (i, b) in binds.iter().enumerate() {
            if i > 0 {
                stmts.push(quote! { __w.separator()?; });
            }
            stmts.push(quote! { ::json_bourne::ToJson::write_json(#b, __w)?; });
        }
        stmts.push(quote! { __w.end_array()?; });
        quote! { #(#stmts)* }
    }
}

//! `FromJson` codegen.

pub(crate) mod enums;
pub(crate) mod object;

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, Ident, Type, spanned::Spanned};

use crate::attrs::ContainerAttrs;
use crate::field_plan::FieldPlan;
use crate::generics::GenericsPlan;
use object::ObjectReader;

pub(crate) struct FromJsonDerive;

impl FromJsonDerive {
    pub(crate) fn expand(input: &DeriveInput) -> syn::Result<TokenStream> {
        let name = &input.ident;
        let container = ContainerAttrs::parse(&input.attrs)?;
        let plan = GenericsPlan::build(input);
        let input_lt = &plan.input_lt;
        let impl_generics = &plan.impl_generics;
        let ty_generics = &plan.ty_generics;
        let where_clause = &plan.where_clause;

        let body = match &input.data {
            Data::Struct(s) => match &s.fields {
                Fields::Named(named) => Self::named(&named.named, &container)?,
                Fields::Unnamed(unnamed) if !unnamed.unnamed.is_empty() => {
                    Self::tuple(&unnamed.unnamed)?
                }
                Fields::Unnamed(_) => {
                    return Err(syn::Error::new(
                        input.span(),
                        "empty tuple structs are unsupported: to_json emits `[]` but \
                         the parser rejects an empty array for a zero-field tuple",
                    ));
                }
                Fields::Unit => {
                    return Err(syn::Error::new(
                        input.span(),
                        "unit structs have no JSON representation",
                    ));
                }
            },
            Data::Enum(e) => enums::EnumReader::expand(name, &e.variants, &container)?,
            Data::Union(_) => {
                return Err(syn::Error::new(input.span(), "unions are unsupported"));
            }
        };

        Ok(quote! {
            impl #impl_generics ::json_bourne::FromJson<#input_lt> for #name #ty_generics
                #where_clause
            {
                fn from_lex(
                    __lex: &mut ::json_bourne::Lexer<#input_lt>,
                ) -> ::core::result::Result<Self, ::json_bourne::Error> {
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
        let body = ObjectReader::read(&quote!(Self), &plans, &container.unknown(), None);
        Ok(quote! { ::core::result::Result::Ok(#body) })
    }

    fn tuple(
        fields: &syn::punctuated::Punctuated<syn::Field, syn::Token![,]>,
    ) -> syn::Result<TokenStream> {
        let n = fields.len();
        if n == 1 {
            let ty = &fields[0].ty;
            return Ok(quote! {
                ::core::result::Result::Ok(Self(
                    <#ty as ::json_bourne::FromJson<'_>>::from_lex(__lex)?,
                ))
            });
        }
        let tys: Vec<&Type> = fields.iter().map(|f| &f.ty).collect();
        let body = Self::tuple_body(&quote!(Self), &tys);
        Ok(quote! { ::core::result::Result::Ok(#body) })
    }

    /// Read a JSON array of exactly `tys.len()` elements into
    /// `ctor(..)`. Shared by tuple structs and tuple variants of every
    /// tagging mode.
    pub(crate) fn tuple_body(ctor: &TokenStream, tys: &[&Type]) -> TokenStream {
        let mut reads = Vec::new();
        let mut idents = Vec::new();
        for (i, ty) in tys.iter().enumerate() {
            let id = Ident::new(&format!("__elem_{i}"), proc_macro2::Span::call_site());
            if i == 0 {
                reads.push(quote! {
                    let #id = <#ty as ::json_bourne::FromJson<'_>>::from_lex(__lex)?;
                });
            } else {
                reads.push(quote! {
                    if __lex.array_continue(b']')? {
                        return ::core::result::Result::Err(::json_bourne::Error::new(
                            ::json_bourne::ErrorKind::TypeMismatch, __lex.position()));
                    }
                    let #id = <#ty as ::json_bourne::FromJson<'_>>::from_lex(__lex)?;
                });
            }
            idents.push(id);
        }
        quote! {{
            if __lex.array_start()? {
                return ::core::result::Result::Err(::json_bourne::Error::new(
                    ::json_bourne::ErrorKind::TypeMismatch, __lex.position()));
            }
            #(#reads)*
            if !__lex.array_continue(b']')? {
                return ::core::result::Result::Err(::json_bourne::Error::new(
                    ::json_bourne::ErrorKind::TypeMismatch, __lex.position()));
            }
            #ctor( #(#idents),* )
        }}
    }
}

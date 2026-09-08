//! `ToJson` codegen for enums: one `match` arm per variant, framed
//! according to the container's tagging mode.

use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::{Ident, Variant, punctuated::Punctuated};

use crate::attrs::{ContainerAttrs, EnumMode, VariantAttrs};
use crate::naming::Naming;
use crate::shape::VShape;
use crate::to_json::ToJsonDerive;
use crate::to_json::object::ObjectWriter;

type Variants = Punctuated<Variant, syn::Token![,]>;

pub(crate) struct EnumWriter;

impl EnumWriter {
    pub(crate) fn expand(
        name: &Ident,
        variants: &Variants,
        container: &ContainerAttrs,
    ) -> syn::Result<TokenStream> {
        let mode = container.enum_mode();
        let mut arms = Vec::new();
        for v in variants {
            let attrs = VariantAttrs::parse(&v.attrs)?;
            let tagkey = Naming::key_expr(&v.ident, &attrs.rename, &container.rename_all);
            arms.push(Self::arm(name, v, &tagkey, &mode, &container.rename_all)?);
        }
        Ok(quote! {
            match self {
                #(#arms)*
            }
        })
    }

    fn bindings(n: usize) -> Vec<Ident> {
        (0..n)
            .map(|i| Ident::new(&format!("__f{i}"), Span::call_site()))
            .collect()
    }

    fn arm(
        name: &Ident,
        v: &Variant,
        tagkey: &TokenStream,
        mode: &EnumMode,
        rename_all: &Option<String>,
    ) -> syn::Result<TokenStream> {
        let vname = &v.ident;
        let shape = VShape::classify(v, rename_all)?;
        Ok(match (mode, &shape) {
            // ---- Untagged ----
            (EnumMode::Untagged, VShape::Unit) => quote! {
                #name::#vname => { __w.write_raw_bytes(b"null")?; ::core::result::Result::Ok(()) }
            },
            (EnumMode::Untagged, VShape::Newtype(_)) => quote! {
                #name::#vname(__inner) => {
                    ::json_bourne::ToJson::write_json(__inner, __w)?;
                    ::core::result::Result::Ok(())
                }
            },
            (EnumMode::Untagged, VShape::Tuple(tys)) => {
                let binds = Self::bindings(tys.len());
                let writes = ToJsonDerive::tuple_payload(&binds);
                quote! {
                    #name::#vname( #(#binds),* ) => { #writes ::core::result::Result::Ok(()) }
                }
            }
            (EnumMode::Untagged, VShape::Struct(fields)) => {
                let pat = ObjectWriter::variant_pattern(fields);
                let body = ObjectWriter::write(fields, false);
                quote! {
                    #name::#vname { #pat } => {
                        __w.begin_object()?;
                        #body
                        __w.end_object()?;
                        ::core::result::Result::Ok(())
                    }
                }
            }

            // ---- External ----
            (EnumMode::External, VShape::Unit) => quote! {
                #name::#vname => {
                    __w.write_escaped_str(#tagkey)?;
                    ::core::result::Result::Ok(())
                }
            },
            (EnumMode::External, VShape::Newtype(_)) => quote! {
                #name::#vname(__inner) => {
                    __w.begin_object()?;
                    __w.object_key(#tagkey)?;
                    ::json_bourne::ToJson::write_json(__inner, __w)?;
                    __w.end_object()?;
                    ::core::result::Result::Ok(())
                }
            },
            (EnumMode::External, VShape::Tuple(tys)) => {
                let binds = Self::bindings(tys.len());
                let writes = ToJsonDerive::tuple_payload(&binds);
                quote! {
                    #name::#vname( #(#binds),* ) => {
                        __w.begin_object()?;
                        __w.object_key(#tagkey)?;
                        #writes
                        __w.end_object()?;
                        ::core::result::Result::Ok(())
                    }
                }
            }
            (EnumMode::External, VShape::Struct(fields)) => {
                let pat = ObjectWriter::variant_pattern(fields);
                let body = ObjectWriter::write(fields, false);
                quote! {
                    #name::#vname { #pat } => {
                        __w.begin_object()?;
                        __w.object_key(#tagkey)?;
                        __w.begin_object()?;
                        #body
                        __w.end_object()?;
                        __w.end_object()?;
                        ::core::result::Result::Ok(())
                    }
                }
            }

            // ---- Internal ----
            (EnumMode::Internal(tag), VShape::Unit) => quote! {
                #name::#vname => {
                    __w.begin_object()?;
                    __w.object_key(#tag)?;
                    __w.write_escaped_str(#tagkey)?;
                    __w.end_object()?;
                    ::core::result::Result::Ok(())
                }
            },
            (EnumMode::Internal(tag), VShape::Struct(fields)) => {
                let pat = ObjectWriter::variant_pattern(fields);
                // The tag is already on the wire, so the first field
                // needs a leading comma like any subsequent member.
                let body = ObjectWriter::write(fields, true);
                quote! {
                    #name::#vname { #pat } => {
                        __w.begin_object()?;
                        __w.object_key(#tag)?;
                        __w.write_escaped_str(#tagkey)?;
                        #body
                        __w.end_object()?;
                        ::core::result::Result::Ok(())
                    }
                }
            }
            (EnumMode::Internal(_), _) => {
                return Err(syn::Error::new(
                    v.ident.span(),
                    "internally-tagged enums support only unit and struct variants",
                ));
            }

            // ---- Adjacent ----
            (EnumMode::Adjacent(tag, _content), VShape::Unit) => quote! {
                #name::#vname => {
                    __w.begin_object()?;
                    __w.object_key(#tag)?;
                    __w.write_escaped_str(#tagkey)?;
                    __w.end_object()?;
                    ::core::result::Result::Ok(())
                }
            },
            (EnumMode::Adjacent(tag, content), VShape::Newtype(_)) => quote! {
                #name::#vname(__inner) => {
                    __w.begin_object()?;
                    __w.object_key(#tag)?;
                    __w.write_escaped_str(#tagkey)?;
                    __w.separator()?;
                    __w.object_key(#content)?;
                    ::json_bourne::ToJson::write_json(__inner, __w)?;
                    __w.end_object()?;
                    ::core::result::Result::Ok(())
                }
            },
            (EnumMode::Adjacent(tag, content), VShape::Tuple(tys)) => {
                let binds = Self::bindings(tys.len());
                let writes = ToJsonDerive::tuple_payload(&binds);
                quote! {
                    #name::#vname( #(#binds),* ) => {
                        __w.begin_object()?;
                        __w.object_key(#tag)?;
                        __w.write_escaped_str(#tagkey)?;
                        __w.separator()?;
                        __w.object_key(#content)?;
                        #writes
                        __w.end_object()?;
                        ::core::result::Result::Ok(())
                    }
                }
            }
            (EnumMode::Adjacent(tag, content), VShape::Struct(fields)) => {
                let pat = ObjectWriter::variant_pattern(fields);
                let body = ObjectWriter::write(fields, false);
                quote! {
                    #name::#vname { #pat } => {
                        __w.begin_object()?;
                        __w.object_key(#tag)?;
                        __w.write_escaped_str(#tagkey)?;
                        __w.separator()?;
                        __w.object_key(#content)?;
                        __w.begin_object()?;
                        #body
                        __w.end_object()?;
                        __w.end_object()?;
                        ::core::result::Result::Ok(())
                    }
                }
            }
        })
    }
}

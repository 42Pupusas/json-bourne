//! The `#[bourne(...)]` attribute model: what a field, a variant, and a
//! container may each say, and the enum tagging mode a container implies.

/// Field-level `#[bourne(...)]` options.
#[derive(Default)]
pub(crate) struct FieldAttrs {
    pub(crate) rename: Option<String>,
    pub(crate) skip: bool,
    pub(crate) default: bool,
    pub(crate) skip_if_none: bool,
}

impl FieldAttrs {
    pub(crate) fn parse(attrs: &[syn::Attribute]) -> syn::Result<Self> {
        let mut out = Self::default();
        for attr in attrs {
            if !attr.path().is_ident("bourne") {
                continue;
            }
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("rename") {
                    let v = meta.value()?;
                    let s: syn::LitStr = v.parse()?;
                    out.rename = Some(s.value());
                } else if meta.path.is_ident("skip") {
                    out.skip = true;
                } else if meta.path.is_ident("default") {
                    out.default = true;
                } else if meta.path.is_ident("skip_if_none") {
                    out.skip_if_none = true;
                } else {
                    return Err(meta.error("unknown bourne field attribute"));
                }
                Ok(())
            })?;
        }
        Ok(out)
    }
}

/// Variant-level `#[bourne(...)]` options. `rename` is the only one.
#[derive(Default)]
pub(crate) struct VariantAttrs {
    pub(crate) rename: Option<String>,
}

impl VariantAttrs {
    pub(crate) fn parse(attrs: &[syn::Attribute]) -> syn::Result<Self> {
        let mut out = Self::default();
        for attr in attrs {
            if !attr.path().is_ident("bourne") {
                continue;
            }
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("rename") {
                    let v = meta.value()?;
                    let s: syn::LitStr = v.parse()?;
                    out.rename = Some(s.value());
                    Ok(())
                } else {
                    Err(meta.error("unsupported variant attribute"))
                }
            })?;
        }
        Ok(out)
    }
}

/// Container-level `#[bourne(...)]` options.
#[derive(Default)]
pub(crate) struct ContainerAttrs {
    pub(crate) rename_all: Option<String>,
    /// `true` when `#[bourne(deny_unknown_fields = false)]` was set —
    /// i.e. unknown keys are skipped rather than erroring.
    pub(crate) lenient: bool,
    pub(crate) tag: Option<String>,
    pub(crate) content: Option<String>,
    pub(crate) untagged: bool,
}

impl ContainerAttrs {
    pub(crate) fn parse(attrs: &[syn::Attribute]) -> syn::Result<Self> {
        let mut out = Self::default();
        for attr in attrs {
            if !attr.path().is_ident("bourne") {
                continue;
            }
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("rename_all") {
                    let v = meta.value()?;
                    let s: syn::LitStr = v.parse()?;
                    out.rename_all = Some(s.value());
                } else if meta.path.is_ident("deny_unknown_fields") {
                    if let Ok(v) = meta.value() {
                        let b: syn::LitBool = v.parse()?;
                        out.lenient = !b.value();
                    } else {
                        out.lenient = false;
                    }
                } else if meta.path.is_ident("tag") {
                    let v = meta.value()?;
                    let s: syn::LitStr = v.parse()?;
                    out.tag = Some(s.value());
                } else if meta.path.is_ident("content") {
                    let v = meta.value()?;
                    let s: syn::LitStr = v.parse()?;
                    out.content = Some(s.value());
                } else if meta.path.is_ident("untagged") {
                    out.untagged = true;
                } else {
                    return Err(meta.error("unknown bourne container attribute"));
                }
                Ok(())
            })?;
        }
        Ok(out)
    }

    /// How a key matching no field is handled. `deny_unknown_fields =
    /// false` opts into skipping; the default is to reject.
    pub(crate) fn unknown(&self) -> crate::from_json::object::Unknown {
        if self.lenient {
            crate::from_json::object::Unknown::Skip
        } else {
            crate::from_json::object::Unknown::Reject
        }
    }

    /// Which tagging scheme an enum uses, validating the combination:
    /// an adjacent enum whose tag and content name the same member emits
    /// that key twice on write and rejects its own output on read, and
    /// `content` without `tag` has no defined wire shape.
    pub(crate) fn enum_mode(&self) -> syn::Result<EnumMode> {
        if self.untagged {
            if self.tag.is_some() || self.content.is_some() {
                return Err(syn::Error::new(
                    proc_macro2::Span::call_site(),
                    "`untagged` cannot be combined with `tag` or `content`",
                ));
            }
            return Ok(EnumMode::Untagged);
        }
        match (&self.tag, &self.content) {
            (Some(t), Some(c)) if t == c => Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                format!(
                    "`tag = \"{t}\"` and `content = \"{t}\"` name the same member; \
                     the writer emits that key twice and the reader rejects it",
                ),
            )),
            (Some(t), Some(c)) => Ok(EnumMode::Adjacent(t.clone(), c.clone())),
            (Some(t), None) => Ok(EnumMode::Internal(t.clone())),
            (None, Some(_)) => Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                "`content` requires a matching `tag`",
            )),
            (None, None) => Ok(EnumMode::External),
        }
    }
}

/// Which tagging scheme an enum uses, derived from container attrs.
pub(crate) enum EnumMode {
    External,
    Internal(String),
    Adjacent(String, String),
    Untagged,
}

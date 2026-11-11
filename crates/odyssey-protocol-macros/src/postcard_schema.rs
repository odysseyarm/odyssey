use proc_macro2::TokenStream;
use quote::quote;
use syn::punctuated::Punctuated;
use syn::{Data, DeriveInput, LitStr, Meta, Token, Type};

fn serde_options(attrs: &[syn::Attribute]) -> syn::Result<Vec<Meta>> {
    let mut options = Vec::new();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("serde")) {
        options.extend(attr.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?);
    }
    Ok(options)
}

fn text(meta: &Meta) -> syn::Result<LitStr> {
    if let Meta::NameValue(value) = meta
        && let syn::Expr::Lit(value) = &value.value
        && let syn::Lit::Str(value) = &value.lit
    {
        return Ok(value.clone());
    }
    Err(syn::Error::new_spanned(meta, "expected string value"))
}

fn field_type(field: &mut syn::Field, needs_alias: &mut bool) -> syn::Result<()> {
    let mut override_type = None;
    for attr in &field.attrs {
        if attr.path().is_ident("postcard_type") {
            override_type = Some(text(&attr.meta)?.parse::<Type>()?);
        }
    }
    for option in serde_options(&field.attrs)? {
        if option.path().is_ident("with") && text(&option)?.value() == "serde_bytes" {
            let Type::Array(array) = &field.ty else {
                return Err(syn::Error::new_spanned(
                    &field.ty,
                    "expected fixed byte array",
                ));
            };
            let mut len = array.len.clone();
            if let syn::Expr::Path(path) = &mut len
                && path
                    .path
                    .segments
                    .first()
                    .is_some_and(|part| part.ident == "Self")
            {
                path.path.segments[0].ident =
                    syn::Ident::new("__PostcardOriginal", proc_macro2::Span::call_site());
                *needs_alias = true;
            }
            override_type = Some(syn::parse_quote!(crate::schema::Bytes<{ #len }>));
        } else if !["default", "deserialize_with"]
            .iter()
            .any(|name| option.path().is_ident(name))
        {
            return Err(syn::Error::new_spanned(
                option,
                "unsupported Serde field schema; model its wire layout explicitly",
            ));
        }
    }
    if let Some(ty) = override_type {
        field.ty = ty;
    }
    field.attrs.clear();
    Ok(())
}

pub(crate) fn expand(mut input: DeriveInput) -> syn::Result<TokenStream> {
    let name = input.ident.clone();
    let mut generics = input.generics.clone();
    for param in generics.type_params_mut() {
        param
            .bounds
            .push(syn::parse_quote!(::postcard_schema_ng::Schema));
    }
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
    let mut from = None;
    let mut into = None;
    let mut transparent = false;
    for option in serde_options(&input.attrs)? {
        if option.path().is_ident("from") {
            from = Some(text(&option)?);
        } else if option.path().is_ident("into") {
            into = Some(text(&option)?);
        } else if option.path().is_ident("transparent") {
            transparent = true;
        } else if !option.path().is_ident("bound") {
            return Err(syn::Error::new_spanned(
                option,
                "unsupported Serde container schema",
            ));
        }
    }
    let schema = if from.is_some() || into.is_some() {
        let (Some(from), Some(into)) = (from, into) else {
            return Err(syn::Error::new_spanned(
                &input,
                "schema requires matching Serde from and into types",
            ));
        };
        if from.value() != into.value() {
            return Err(syn::Error::new_spanned(
                into,
                "Serde from and into wire types differ",
            ));
        }
        let ty = from.parse::<Type>()?;
        quote!(<#ty as ::postcard_schema_ng::Schema>::SCHEMA)
    } else {
        let mut needs_alias = false;
        match &mut input.data {
            Data::Struct(data) => {
                for field in &mut data.fields {
                    field_type(field, &mut needs_alias)?;
                }
            }
            Data::Enum(data) => {
                for variant in &mut data.variants {
                    if !serde_options(&variant.attrs)?.is_empty() {
                        return Err(syn::Error::new_spanned(
                            variant,
                            "Serde variant overrides need an explicit wire schema",
                        ));
                    }
                    variant.attrs.clear();
                    for field in &mut variant.fields {
                        field_type(field, &mut needs_alias)?;
                    }
                }
            }
            Data::Union(_) => {
                return Err(syn::Error::new_spanned(
                    &input,
                    "unions have no wire schema",
                ));
            }
        }
        if transparent {
            let Data::Struct(data) = &input.data else {
                return Err(syn::Error::new_spanned(
                    &input,
                    "transparent schema requires a struct",
                ));
            };
            if data.fields.len() != 1 {
                return Err(syn::Error::new_spanned(
                    &data.fields,
                    "transparent schema requires one field",
                ));
            }
            let ty = &data.fields.iter().next().expect("one field").ty;
            quote!(<#ty as ::postcard_schema_ng::Schema>::SCHEMA)
        } else {
            input.attrs.clear();
            input.vis = syn::Visibility::Inherited;
            if let Data::Struct(data) = &mut input.data {
                for field in &mut data.fields {
                    field.vis = syn::Visibility::Inherited;
                }
            }
            let alias = needs_alias.then(|| quote!(use self::#name as __PostcardOriginal;));
            quote!({
                #alias
                #[allow(
                    dead_code,
                    clippy::empty_enum_variants_with_brackets,
                    clippy::large_enum_variant,
                    clippy::struct_field_names,
                    reason = "unconstructed schema mirrors must preserve the public wire shape"
                )]
                #[derive(::postcard_schema_ng::Schema)]
                #input
                <#name #ty_generics as ::postcard_schema_ng::Schema>::SCHEMA
            })
        }
    };
    Ok(quote! {
        impl #impl_generics ::postcard_schema_ng::Schema for #name #ty_generics #where_clause {
            const SCHEMA: &'static ::postcard_schema_ng::schema::DataModelType = #schema;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::expand;

    #[test]
    fn unmodeled_serde_layout_changes_fail_closed() {
        for source in [
            r#"#[serde(tag = "kind")] enum Packet { Ack }"#,
            "enum Packet { #[serde(skip_deserializing)] Retired, Future }",
            "struct Packet { #[serde(skip)] id: u8 }",
            r#"struct Packet { #[serde(serialize_with = "custom")] id: u8 }"#,
            r#"#[serde(from = "u8", into = "u16")] struct Packet(u8);"#,
        ] {
            assert!(
                expand(syn::parse_str(source).expect("valid declaration")).is_err(),
                "{source}"
            );
        }
    }
}

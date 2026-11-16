//! Attribute macros for `odyssey-protocol`.
//!
//! Every wire type repeats the same feature-gated derives. `protocol_type`
//! prepends them so a type only spells out what differs.

use proc_macro::TokenStream;
use quote::quote;

mod postcard_schema;

#[proc_macro_derive(PostcardSchema, attributes(postcard_type))]
pub fn postcard_schema(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as syn::DeriveInput);
    postcard_schema::expand(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Derives `defmt::Format`, serde and minicbor, each behind its feature.
///
/// `no_cbor` skips minicbor, for types another type carries through a
/// `serde(from = ..., into = ...)`.
///
/// ```ignore
/// #[protocol_type]
/// #[repr(C)]
/// #[derive(Clone, Copy, Debug, PartialEq)]
/// pub struct Version {
///     #[cfg_attr(feature = "minicbor", n(0))]
///     pub major: u8,
/// }
/// ```
///
/// # Since
/// Odyssey Desktop 0.6.0.
#[proc_macro_attribute]
pub fn protocol_type(args: TokenStream, item: TokenStream) -> TokenStream {
    let cbor = match args.to_string().trim() {
        "" => true,
        "no_cbor" => false,
        other => {
            let message = format!("unknown protocol_type argument: {other}");
            return quote!(compile_error!(#message);).into();
        }
    };

    let minicbor = cbor.then(|| {
        quote! {
            #[cfg_attr(
                feature = "minicbor",
                derive(::minicbor::Encode, ::minicbor::Decode, ::minicbor::CborLen)
            )]
        }
    });
    let item = proc_macro2::TokenStream::from(item);

    quote! {
        #[cfg_attr(feature = "defmt", derive(defmt::Format))]
        #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
        #[cfg_attr(feature = "schema", derive(::odyssey_protocol_macros::PostcardSchema))]
        #minicbor
        #item
    }
    .into()
}

//! Attribute macros for `odyssey-protocol`.
//!
//! Every wire type in that crate repeats the same feature-gated derive stack.
//! These macros prepend it, so a type declares only what makes it different:
//! its `repr`, its pyo3 class form, and its own derives.

use proc_macro::TokenStream;
use quote::quote;

/// Derives `defmt::Format`, serde, and minicbor on a wire type, each behind its
/// feature.
///
/// Pass `no_cbor` for the types that are not encoded with minicbor directly —
/// usually because a `serde(from = ..., into = ...)` conversion carries them
/// through another type.
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
        #minicbor
        #item
    }
    .into()
}

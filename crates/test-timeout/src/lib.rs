use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, Attribute, ItemFn, LitInt};

#[proc_macro_attribute]
pub fn tokio_timeout_test(attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut timeout_secs: u64 = 60;

    if !attr.is_empty() {
        let lit = parse_macro_input!(attr as LitInt);
        timeout_secs = lit
            .base10_parse()
            .unwrap_or_else(|err| panic!("invalid timeout value: {err}"));
        if timeout_secs == 0 {
            panic!("timeout must be greater than zero");
        }
    }

    let ItemFn {
        attrs,
        vis,
        sig,
        block,
    } = parse_macro_input!(item as ItemFn);

    if sig.asyncness.is_none() {
        return syn::Error::new_spanned(
            &sig.ident,
            "tokio_timeout_test can only be applied to async functions",
        )
        .to_compile_error()
        .into();
    }

    let filtered_attrs: Vec<Attribute> = attrs
        .into_iter()
        .filter(|attr| !is_tokio_test_attribute(attr))
        .collect();

    let timeout = timeout_secs;

    TokenStream::from(quote! {
        #[tokio::test]
        #(#filtered_attrs)*
        #vis #sig {
            let fut = async move #block;
            tokio::time::timeout(std::time::Duration::from_secs(#timeout), fut)
                .await
                .expect("test timed out")
        }
    })
}

fn is_tokio_test_attribute(attr: &Attribute) -> bool {
    let mut segments = attr.path().segments.iter();
    matches!(
        (segments.next(), segments.next(), segments.next()),
        (Some(first), Some(second), None)
            if first.ident == "tokio" && second.ident == "test"
    )
}

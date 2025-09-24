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
        mut sig,
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

    sig.asyncness = None;

    let filtered_attrs: Vec<Attribute> = attrs
        .into_iter()
        .filter(|attr| !is_tokio_test_attribute(attr))
        .collect();

    let timeout = timeout_secs;

    TokenStream::from(quote! {
        #[test]
        #(#filtered_attrs)*
        #vis #sig {
            let timeout_duration = std::time::Duration::from_secs(#timeout);
            let (sender, receiver) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("failed to build Tokio runtime");
                    runtime.block_on(async {
                        tokio::time::timeout(timeout_duration, async move #block)
                            .await
                            .expect("test timed out");
                    });
                }));
                let _ = sender.send(result);
            });
            match receiver.recv_timeout(timeout_duration) {
                Ok(Ok(_)) => {}
                Ok(Err(payload)) => std::panic::resume_unwind(payload),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => panic!("test timed out"),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("test thread failed before reporting result")
                }
            }
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

#[proc_macro_attribute]
pub fn timeout(attr: TokenStream, item: TokenStream) -> TokenStream {
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

    if sig.asyncness.is_some() {
        return syn::Error::new_spanned(
            &sig.ident,
            "timeout attribute expects a synchronous test function",
        )
        .to_compile_error()
        .into();
    }

    let filtered_attrs: Vec<Attribute> = attrs
        .into_iter()
        .filter(|attr| !is_test_attribute(attr))
        .collect();

    let timeout = timeout_secs;

    TokenStream::from(quote! {
        #[test]
        #(#filtered_attrs)*
        #vis #sig {
            let timeout_duration = std::time::Duration::from_secs(#timeout);
            let (sender, receiver) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| #block ));
                let _ = sender.send(result);
            });
            match receiver.recv_timeout(timeout_duration) {
                Ok(Ok(_)) => {}
                Ok(Err(payload)) => std::panic::resume_unwind(payload),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => panic!("test timed out"),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("test thread failed before reporting result")
                }
            }
        }
    })
}

fn is_test_attribute(attr: &Attribute) -> bool {
    let mut segments = attr.path().segments.iter();
    matches!((segments.next(), segments.next()), (Some(first), None) if first.ident == "test")
}

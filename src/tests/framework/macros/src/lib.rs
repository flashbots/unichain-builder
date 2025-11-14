use {
	proc_macro::TokenStream,
	quote::quote,
	syn::{Expr, ItemFn, Meta, Token, parse_macro_input, punctuated::Punctuated},
};

struct TestConfig {
	args: Option<Expr>,
}

impl syn::parse::Parse for TestConfig {
	fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
		let mut config = TestConfig { args: None };

		let args: Punctuated<Meta, Token![,]> =
			input.parse_terminated(Meta::parse, Token![,])?;

		for arg in args {
			match arg {
				Meta::NameValue(nv) => {
					if let Some(ident) = nv.path.get_ident() {
						let name = ident.to_string();
						if name == "args" {
							config.args = Some(nv.value);
						} else {
							return Err(syn::Error::new_spanned(
								nv.path,
								format!("Unknown attribute '{}', 'args', or ''", name),
							));
						}
					}
				}
				_ => {
					return Err(syn::Error::new_spanned(
						arg,
						"Invalid attribute format.".to_string(),
					));
				}
			}
		}

		Ok(config)
	}
}

#[proc_macro_attribute]
pub fn unichain_test(args: TokenStream, input: TokenStream) -> TokenStream {
	let mut input_fn = parse_macro_input!(input as ItemFn);
	let config = parse_macro_input!(args as TestConfig);

	let harness_init: proc_macro2::TokenStream = if let Some(args) = config.args {
		quote! { crate::tests::Harness::try_new(#args).await? }
	} else {
		quote! { crate::tests::Harness::default().await? }
	};

	// Rename the original test function and use the original name for the
	// generated test
	let helper_name_str = format!("{}_impl", input_fn.sig.ident);
	let helper_name_ident =
		syn::Ident::new(&helper_name_str, input_fn.sig.ident.span());
	let original_name = input_fn.sig.ident.clone();
	input_fn.sig.ident = helper_name_ident.clone();

	quote! {
		#input_fn

		#[tokio::test]
		async fn #original_name() -> eyre::Result<()> {
			let subscriber = tracing_subscriber::fmt()
				.with_env_filter(std::env::var("RUST_LOG")
					.unwrap_or_else(|_| "info".to_string()))
				.with_file(true)
				.with_line_number(true)
				.with_test_writer()
				.finish();
			let _guard = tracing::subscriber::set_global_default(subscriber);

			let harness = #harness_init;
			#helper_name_ident(harness).await
		}
	}
	.into()
}

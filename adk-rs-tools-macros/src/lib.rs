//! `#[adk::tool]` proc-macro.
//!
//! Usage:
//!
//! ```ignore
//! use adk_rs_core::ToolContext;
//! use adk_rs_error::Result;
//! use serde::{Deserialize, Serialize};
//! use schemars::JsonSchema;
//!
//! #[derive(Deserialize, JsonSchema)]
//! struct GetWeatherArgs {
//!     /// City name.
//!     city: String,
//! }
//!
//! #[derive(Serialize)]
//! struct WeatherReport { temp_c: f32 }
//!
//! #[adk_rs_tools_macros::tool]
//! /// Look up the weather in `args.city`.
//! async fn get_weather(args: GetWeatherArgs, _ctx: &mut ToolContext) -> Result<WeatherReport> {
//!     Ok(WeatherReport { temp_c: 22.0 })
//! }
//! ```
//!
//! The macro emits a unit struct named after the function (PascalCased) and a
//! free constructor `get_weather() -> Arc<dyn adk_rs_core::DynTool>`. The args
//! struct must implement `serde::Deserialize` and `schemars::JsonSchema`.

#![forbid(unsafe_code)]

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote;
use syn::{FnArg, ItemFn, Pat, PatType, Type, parse_macro_input, parse_quote};

fn upper_camel(s: &str) -> String {
    let mut out = String::new();
    let mut up = true;
    for c in s.chars() {
        if c == '_' {
            up = true;
        } else if up {
            out.push(c.to_ascii_uppercase());
            up = false;
        } else {
            out.push(c);
        }
    }
    out
}

fn doc_comment(attrs: &[syn::Attribute]) -> String {
    let mut out = String::new();
    for a in attrs {
        if a.path().is_ident("doc") {
            if let syn::Meta::NameValue(nv) = a.meta.clone() {
                if let syn::Expr::Lit(lit) = nv.value {
                    if let syn::Lit::Str(s) = lit.lit {
                        if !out.is_empty() {
                            out.push('\n');
                        }
                        out.push_str(s.value().trim());
                    }
                }
            }
        }
    }
    out
}

#[proc_macro_attribute]
/// `#[adk::tool]` — see crate docs.
pub fn tool(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let f = parse_macro_input!(item as ItemFn);
    let vis = &f.vis;
    let name_ident = &f.sig.ident;
    let name_str = name_ident.to_string();
    let pascal = upper_camel(&name_str);
    let struct_ident = syn::Ident::new(&pascal, Span::call_site());
    let description = doc_comment(&f.attrs);

    if f.sig.asyncness.is_none() {
        return TokenStream::from(quote! {
            compile_error!("#[adk_rs_tools_macros::tool] requires an async fn");
        });
    }

    // Inspect arguments: expect (args: ArgsType, ctx: &mut ToolContext)
    let inputs: Vec<&FnArg> = f.sig.inputs.iter().collect();
    if inputs.len() != 2 {
        return TokenStream::from(quote! {
            compile_error!("#[adk_rs_tools_macros::tool] requires exactly two args: (args: T, ctx: &mut ToolContext)");
        });
    }
    let PatType {
        pat: arg_pat,
        ty: arg_ty,
        ..
    } = match inputs[0] {
        FnArg::Typed(p) => p.clone(),
        FnArg::Receiver(_) => {
            return TokenStream::from(quote! {
                compile_error!("#[adk_rs_tools_macros::tool] doesn't support receivers");
            });
        }
    };
    let arg_ident = match *arg_pat {
        Pat::Ident(ref id) => id.ident.clone(),
        _ => {
            return TokenStream::from(
                quote! { compile_error!("first arg must be a simple identifier"); },
            );
        }
    };

    let PatType {
        pat: ctx_pat,
        ty: _ctx_ty,
        ..
    } = match inputs[1] {
        FnArg::Typed(p) => p.clone(),
        FnArg::Receiver(_) => {
            return TokenStream::from(quote! {
                compile_error!("#[adk_rs_tools_macros::tool] doesn't support receivers");
            });
        }
    };
    let ctx_ident = match *ctx_pat {
        Pat::Ident(ref id) => id.ident.clone(),
        _ => {
            return TokenStream::from(
                quote! { compile_error!("second arg must be a simple identifier"); },
            );
        }
    };

    // We need the args type to implement schemars::JsonSchema; we'll call
    // `schemars::schema_for!` on it at runtime via a helper.
    let arg_ty_owned: Type = parse_quote!(#arg_ty);

    let body = &f.block;
    let ret_ty = &f.sig.output;

    let constructor_name = name_ident.clone();

    let expanded = quote! {
        #[doc = #description]
        #[derive(Debug, Default, Clone, Copy)]
        #vis struct #struct_ident;

        #[::async_trait::async_trait]
        impl ::adk_rs_core::DynTool for #struct_ident {
            fn name(&self) -> &str { #name_str }
            fn description(&self) -> &str { #description }
            fn declaration(&self) -> ::std::option::Option<::adk_rs_genai_types::FunctionDeclaration> {
                let root = ::schemars::schema_for!(#arg_ty_owned);
                let schema = ::adk_rs_genai_types::Schema::from_schemars(&root)
                    .unwrap_or_else(|_| ::adk_rs_genai_types::Schema::object());
                ::std::option::Option::Some(
                    ::adk_rs_genai_types::FunctionDeclaration::new(#name_str, #description)
                        .with_parameters(schema),
                )
            }
            async fn run(
                &self,
                args: ::serde_json::Value,
                #ctx_ident: &mut ::adk_rs_core::ToolContext,
            ) -> ::adk_rs_error::Result<::serde_json::Value> {
                async fn __inner(#arg_ident: #arg_ty_owned, #ctx_ident: &mut ::adk_rs_core::ToolContext) #ret_ty #body
                let typed: #arg_ty_owned = ::serde_json::from_value(args).map_err(|e| {
                    ::adk_rs_error::Error::Tool(::adk_rs_error::ToolError::InvalidArgs {
                        tool: #name_str.to_string(),
                        message: e.to_string(),
                    })
                })?;
                let r = __inner(typed, #ctx_ident).await?;
                ::serde_json::to_value(r).map_err(|e| {
                    ::adk_rs_error::Error::Tool(::adk_rs_error::ToolError::Execution {
                        tool: #name_str.to_string(),
                        message: e.to_string(),
                    })
                })
            }
        }

        /// Construct the tool.
        #vis fn #constructor_name() -> ::std::sync::Arc<dyn ::adk_rs_core::DynTool> {
            ::std::sync::Arc::new(#struct_ident)
        }
    };

    TokenStream::from(expanded)
}

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::{
    DeriveInput, Expr, ExprCall, FnArg, Ident, ItemFn, LitStr, Pat, Result as SynResult,
    ReturnType, Token, Type, parenthesized, parse_macro_input,
};

#[proc_macro_attribute]
pub fn component(attr: TokenStream, item: TokenStream) -> TokenStream {
    let config = parse_macro_input!(attr as ComponentConfig);
    let function = parse_macro_input!(item as ItemFn);
    let export = function.sig.ident.clone();
    let ComponentConfig {
        world,
        path,
        component,
    } = config;
    let component_items = component_items(&world, path.as_ref(), &[export], &component);

    match expand_callable(function) {
        Ok(callable) => quote! {
            #callable

            #component_items
        }
        .into(),
        Err(error) => error.to_compile_error().into(),
    }
}

#[proc_macro_attribute]
pub fn fail_child_component(attr: TokenStream, item: TokenStream) -> TokenStream {
    let config = parse_macro_input!(attr as ComponentConfig);
    let function = parse_macro_input!(item as ItemFn);
    let export = function.sig.ident.clone();
    let ComponentConfig {
        world,
        path,
        component,
    } = config.with_default_world("fail-child");
    let component_items = fail_child_component_items(&world, path.as_ref(), &export, &component);

    quote! {
        #function

        #component_items
    }
    .into()
}

#[proc_macro_attribute]
pub fn fail_supervisor_component(attr: TokenStream, item: TokenStream) -> TokenStream {
    let config = parse_macro_input!(attr as ComponentConfig);
    let function = parse_macro_input!(item as ItemFn);
    let export = function.sig.ident.clone();
    let ComponentConfig {
        world,
        path,
        component,
    } = config.with_default_world("fail-supervisor");
    let component_items =
        fail_supervisor_component_items(&world, path.as_ref(), &export, &component);

    quote! {
        #function

        #component_items
    }
    .into()
}

fn component_items(
    world: &LitStr,
    path: Option<&LitStr>,
    exports: &[Ident],
    component: &Ident,
) -> proc_macro2::TokenStream {
    let wit_source = match path {
        Some(path) => quote! { path: #path, },
        None => {
            let wit = component_wit(world, exports);
            quote! { inline: #wit, }
        }
    };

    quote! {
        ::wit_bindgen::generate!({
            world: #world,
            #wit_source
            runtime_path: "::wit_bindgen::rt",
        });

        #[derive(runtime::Guest)]
        #[runtime(exports(#(#exports),*))]
        struct #component;

        export!(#component);
    }
}

fn fail_child_component_items(
    world: &LitStr,
    path: Option<&LitStr>,
    export: &Ident,
    component: &Ident,
) -> proc_macro2::TokenStream {
    let wit_source = match path {
        Some(path) => quote! { path: #path, },
        None => {
            let wit = fail_child_wit(world, export);
            quote! { inline: #wit, }
        }
    };

    quote! {
        ::wit_bindgen::generate!({
            world: #world,
            #wit_source
            runtime_path: "::wit_bindgen::rt",
        });

        struct #component;

        impl Guest for #component {
            fn init(burn_iters: u64) {
                init(burn_iters)
            }

            fn #export(n: u64) -> u64 {
                #export(n)
            }
        }

        export!(#component);
    }
}

fn fail_supervisor_component_items(
    world: &LitStr,
    path: Option<&LitStr>,
    export: &Ident,
    component: &Ident,
) -> proc_macro2::TokenStream {
    let wit_source = match path {
        Some(path) => quote! { path: #path, },
        None => {
            let wit = fail_supervisor_wit(world, export);
            quote! { inline: #wit, }
        }
    };

    quote! {
        ::wit_bindgen::generate!({
            world: #world,
            #wit_source
            runtime_path: "::wit_bindgen::rt",
        });

        struct #component;

        impl Guest for #component {
            fn #export(n: u64, max_retries: u32) -> (i32, u32, i32, u64) {
                let report = #export(n, max_retries);
                (
                    report.status,
                    report.attempts,
                    report.child_status,
                    report.result,
                )
            }
        }

        export!(#component);
    }
}

#[proc_macro_derive(Guest, attributes(runtime))]
pub fn derive_guest(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    match expand_guest(&input) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

#[proc_macro_attribute]
pub fn callable(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let function = parse_macro_input!(item as ItemFn);
    match expand_callable(function) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

fn expand_callable(function: ItemFn) -> SynResult<proc_macro2::TokenStream> {
    let name = function.sig.ident.clone();
    let function_name = wit_function_name(&name);
    let module_name = format_ident!("__runtime_call_{name}");

    let Some(arg) = single_u64_arg(&function) else {
        return Err(syn::Error::new_spanned(
            function.sig.inputs,
            "runtime callables currently support exactly one u64 parameter",
        ));
    };
    if !returns_u64(&function) {
        return Err(syn::Error::new_spanned(
            function.sig.output,
            "runtime callables currently support u64 return type",
        ));
    }

    Ok(quote! {
        #function

        #[allow(non_snake_case)]
        pub(crate) mod #module_name {
            pub(crate) struct Call {
                #arg: u64,
            }

            impl Call {
                pub(crate) fn new(#arg: u64) -> Self {
                    Self { #arg }
                }
            }

            impl ::runtime::ComponentCall for Call {
                type Output = u64;

                const FUNCTION: &'static str = #function_name;

                fn encode(self) -> Vec<u8> {
                    ::runtime::encode_u64(self.#arg)
                }

                fn decode(payload: Vec<u8>) -> Result<Self::Output, ::runtime::ChildStatus> {
                    ::runtime::decode_u64(payload)
                }
            }
        }
    })
}

#[proc_macro]
pub fn spawn(input: TokenStream) -> TokenStream {
    let call = parse_macro_input!(input as ExprCall);
    let Expr::Path(path) = *call.func else {
        return compile_error("runtime::spawn! expects a direct function call like fib(n)");
    };
    let Some(segment) = path.path.segments.last() else {
        return compile_error("runtime::spawn! expects a function name");
    };

    let module_name = format_ident!("__runtime_call_{}", segment.ident);
    let args = call.args;

    quote! {
        ::runtime::spawn_call(crate::#module_name::Call::new(#args))
    }
    .into()
}

#[proc_macro]
pub fn spawn_link(input: TokenStream) -> TokenStream {
    let input = proc_macro2::TokenStream::from(input);
    quote! {
        crate::rust_wasm_runtime::supervisor::runtime::spawn_link(#input)
    }
    .into()
}

struct ComponentConfig {
    world: LitStr,
    path: Option<LitStr>,
    component: Ident,
}

impl Parse for ComponentConfig {
    fn parse(input: ParseStream<'_>) -> SynResult<Self> {
        let mut world = None;
        let mut path = None;
        let mut component = None;

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            parse_key_value_separator(input)?;

            match key.to_string().as_str() {
                "world" => set_once(&mut world, input.parse()?, &key)?,
                "path" => set_once(&mut path, input.parse()?, &key)?,
                "component" => set_once(&mut component, input.parse()?, &key)?,
                _ => {
                    return Err(syn::Error::new(
                        key.span(),
                        "expected one of: world, path, component",
                    ));
                }
            }

            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }

        Ok(Self {
            world: world.unwrap_or_else(|| LitStr::new("fib-guest", Span::call_site())),
            path,
            component: component.unwrap_or_else(|| Ident::new("Component", Span::call_site())),
        })
    }
}

impl ComponentConfig {
    fn with_default_world(self, default_world: &str) -> Self {
        let default_fib_world = LitStr::new("fib-guest", Span::call_site());
        let world = if self.world.value() == default_fib_world.value() {
            LitStr::new(default_world, Span::call_site())
        } else {
            self.world
        };

        Self { world, ..self }
    }
}

fn component_wit(world: &LitStr, exports: &[Ident]) -> LitStr {
    let exports = exports
        .iter()
        .map(|export| {
            format!(
                "    export {}: async func(n: u64) -> u64;\n",
                wit_function_name(export)
            )
        })
        .collect::<String>();
    let wit = format!(
        r#"package rust-wasm-runtime:process;

interface runtime {{
    resource task;

    enum child-status {{
        stack-overflow,
        trap,
        bad-export,
        runtime-error,
    }}

    spawn: func(function: string, payload: list<u8>) -> task;
    await-task: async func(task: task) -> result<list<u8>, child-status>;
}}

world {} {{
    import runtime;

{exports}}}
"#,
        world.value()
    );

    LitStr::new(&wit, Span::call_site())
}

fn fail_child_wit(world: &LitStr, export: &Ident) -> LitStr {
    let wit = format!(
        r#"package rust-wasm-runtime:child;

world {} {{
    export init: func(burn-iters: u64);
    export {}: func(n: u64) -> u64;
}}
"#,
        world.value(),
        wit_function_name(export)
    );

    LitStr::new(&wit, Span::call_site())
}

fn fail_supervisor_wit(world: &LitStr, export: &Ident) -> LitStr {
    let wit = format!(
        r#"package rust-wasm-runtime:supervisor;

interface runtime {{
    spawn-link: func(n: u64) -> tuple<s32, u64>;
}}

world {} {{
    import runtime;

    export {}: func(n: u64, max-retries: u32) -> tuple<s32, u32, s32, u64>;
}}
"#,
        world.value(),
        wit_function_name(export)
    );

    LitStr::new(&wit, Span::call_site())
}

fn wit_function_name(ident: &Ident) -> String {
    ident.to_string().replace('_', "-")
}

fn parse_key_value_separator(input: ParseStream<'_>) -> SynResult<()> {
    if input.peek(Token![=]) {
        input.parse::<Token![=]>()?;
    } else {
        input.parse::<Token![:]>()?;
    }
    Ok(())
}

fn set_once<T>(slot: &mut Option<T>, value: T, key: &Ident) -> SynResult<()> {
    if slot.replace(value).is_some() {
        return Err(syn::Error::new(
            key.span(),
            "duplicate component config key",
        ));
    }
    Ok(())
}

fn expand_guest(input: &DeriveInput) -> SynResult<proc_macro2::TokenStream> {
    let component = &input.ident;
    let exports = runtime_exports(input)?;
    let methods = exports.iter().map(|export| {
        quote! {
            async fn #export(n: u64) -> u64 {
                #export(n).await
            }
        }
    });
    let (impl_generics, type_generics, where_clause) = input.generics.split_for_impl();

    Ok(quote! {
        impl #impl_generics Guest for #component #type_generics #where_clause {
            #(#methods)*
        }
    })
}

fn runtime_exports(input: &DeriveInput) -> SynResult<Vec<Ident>> {
    let mut exports: Option<Vec<Ident>> = None;

    for attr in &input.attrs {
        if !attr.path().is_ident("runtime") {
            continue;
        }

        attr.parse_nested_meta(|meta| {
            if !meta.path.is_ident("exports") {
                return Err(meta.error("expected runtime(exports(...))"));
            }
            if exports.is_some() {
                return Err(meta.error("duplicate runtime exports"));
            }

            let content;
            parenthesized!(content in meta.input);
            let parsed = content.parse_terminated(Ident::parse, Token![,])?;
            exports = Some(parsed.into_iter().collect());
            Ok(())
        })?;
    }

    let exports = exports.ok_or_else(|| {
        syn::Error::new(
            input.ident.span(),
            "missing #[runtime(exports(function_name, ...))]",
        )
    })?;
    if exports.is_empty() {
        return Err(syn::Error::new(
            input.ident.span(),
            "runtime exports cannot be empty",
        ));
    }

    Ok(exports)
}

fn single_u64_arg(function: &ItemFn) -> Option<syn::Ident> {
    let mut inputs = function.sig.inputs.iter();
    let arg = inputs.next()?;
    if inputs.next().is_some() {
        return None;
    }

    let FnArg::Typed(arg) = arg else {
        return None;
    };
    if !is_u64(&arg.ty) {
        return None;
    }

    match &*arg.pat {
        Pat::Ident(ident) => Some(ident.ident.clone()),
        _ => None,
    }
}

fn returns_u64(function: &ItemFn) -> bool {
    match &function.sig.output {
        ReturnType::Type(_, ty) => is_u64(ty),
        ReturnType::Default => false,
    }
}

fn is_u64(ty: &Type) -> bool {
    match ty {
        Type::Path(path) => path
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "u64"),
        _ => false,
    }
}

fn compile_error(message: &str) -> TokenStream {
    quote! {
        compile_error!(#message);
    }
    .into()
}

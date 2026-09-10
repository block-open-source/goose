use proc_macro::TokenStream;
use quote::quote;
use syn::{
    parse_macro_input, FnArg, GenericArgument, ImplItem, ItemImpl, Pat, PathArguments, ReturnType,
    Type,
};

/// Marks an impl block as containing `#[custom_method(RequestType)]`-annotated handlers.
///
/// The request type must derive `agent_client_protocol::JsonRpcRequest` with a `#[request(method = "...")]`
/// attribute — the method name is extracted from that type at compile time, eliminating
/// duplication between the request struct and the handler.
///
/// Generates two methods on the impl:
///
/// 1. `handle_custom_request` — a dispatcher that:
///    - Uses `<RequestType as agent_client_protocol::JsonRpcMessage>::matches_method` to match incoming methods
///    - Parses JSON params into the handler's typed parameter (if any)
///    - Serializes the handler's return value to JSON
///
/// 2. `custom_method_schemas` — returns a `Vec<CustomMethodSchema>` with
///    JSON Schema for each method's params and response types. Types that
///    implement `schemars::JsonSchema` get a full schema; `serde_json::Value`
///    params/responses produce `None`.
///
/// # Handler signatures
///
/// Handlers may take zero or one request parameter (beyond `&self`). A handler
/// that needs the active ACP connection may take it before the request:
///
/// ```ignore
/// // No params — called for requests with no/empty params
/// #[custom_method(GetExtensionsRequest)]
/// async fn on_get_extensions(&self) -> Result<GetExtensionsResponse, agent_client_protocol::Error> { .. }
///
/// // Typed params — JSON params auto-deserialized
/// #[custom_method(GetSessionRequest)]
/// async fn on_get_session(&self, req: GetSessionRequest) -> Result<GetSessionResponse, agent_client_protocol::Error> { .. }
///
/// // Connection and typed params
/// #[custom_method(StartCallRequest)]
/// async fn on_start_call(
///     &self,
///     cx: &agent_client_protocol::ConnectionTo<agent_client_protocol::Client>,
///     req: StartCallRequest,
/// ) -> Result<StartCallResponse, agent_client_protocol::Error> { .. }
/// ```
///
/// The return type must be `Result<T, agent_client_protocol::Error>` where `T: Serialize`.
#[proc_macro_attribute]
pub fn custom_methods(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut impl_block = parse_macro_input!(item as ItemImpl);

    let mut routes: Vec<Route> = Vec::new();

    // Collect all #[custom_method(RequestType)] annotations and strip them.
    for item in &mut impl_block.items {
        if let ImplItem::Fn(method) = item {
            let mut request_type = None;
            method.attrs.retain(|attr| {
                if attr.path().is_ident("custom_method") {
                    if let Ok(meta_list) = attr.meta.require_list() {
                        if let Ok(ty) = meta_list.parse_args::<Type>() {
                            request_type = Some(ty);
                        }
                    }
                    false // strip the attribute
                } else {
                    true // keep other attributes
                }
            });

            if let Some(req_type) = request_type {
                let fn_ident = method.sig.ident.clone();

                let parameter_types = extract_parameter_types(&method.sig);
                let (uses_connection, param_type) = match parameter_types.as_slice() {
                    [] => (false, None),
                    [request] => (false, Some(request.clone())),
                    [_connection, request] => (true, Some(request.clone())),
                    _ => panic!("custom method handlers accept at most a connection and request"),
                };
                let return_type = extract_return_type(&method.sig);
                let ok_type = extract_result_ok_type(&method.sig);

                routes.push(Route {
                    request_type: req_type,
                    fn_ident,
                    param_type,
                    uses_connection,
                    return_type,
                    ok_type,
                });
            }
        }
    }

    // Generate the dispatch arms using matches_method for routing.
    let arms: Vec<_> = routes
        .iter()
        .map(|route| {
            let req_type = &route.request_type;
            let fn_ident = &route.fn_ident;
            let call = if route.uses_connection {
                quote! { self.#fn_ident(cx, req).await? }
            } else {
                quote! { self.#fn_ident(req).await? }
            };

            match &route.param_type {
                Some(_) => {
                    quote! {
                        if <#req_type as agent_client_protocol::JsonRpcMessage>::matches_method(method) {
                            let req = serde_json::from_value(params)
                                .map_err(|e| agent_client_protocol::Error::invalid_params().data(e.to_string()))?;
                            let result = #call;
                            return serde_json::to_value(&result)
                                .map_err(|e| agent_client_protocol::Error::internal_error().data(e.to_string()));
                        }
                    }
                }
                None => {
                    quote! {
                        if <#req_type as agent_client_protocol::JsonRpcMessage>::matches_method(method) {
                            let result = self.#fn_ident().await?;
                            return serde_json::to_value(&result)
                                .map_err(|e| agent_client_protocol::Error::internal_error().data(e.to_string()));
                        }
                    }
                }
            }
        })
        .collect();

    // Generate schema entries for each route using SchemaGenerator for $ref dedup.
    let schema_entries: Vec<_> = routes
        .iter()
        .map(|route| {
            let req_type = &route.request_type;

            let params_expr = if let Some(pt) = &route.param_type {
                if is_json_value(pt) {
                    quote! { None }
                } else {
                    quote! { Some(generator.subschema_for::<#pt>()) }
                }
            } else {
                // Even with no handler param, generate schema from the request type
                if is_json_value(req_type) {
                    quote! { None }
                } else {
                    quote! { Some(generator.subschema_for::<#req_type>()) }
                }
            };

            let response_expr = if let Some(ok_ty) = &route.ok_type {
                if is_json_value(ok_ty) {
                    quote! { None }
                } else {
                    quote! { Some(generator.subschema_for::<#ok_ty>()) }
                }
            } else {
                quote! { None }
            };

            let params_name_expr = if let Some(pt) = &route.param_type {
                if is_json_value(pt) {
                    quote! { None }
                } else {
                    let name = type_name(pt);
                    quote! { Some(#name.to_string()) }
                }
            } else {
                let name = type_name(req_type);
                quote! { Some(#name.to_string()) }
            };

            let response_name_expr = if let Some(ok_ty) = &route.ok_type {
                if is_json_value(ok_ty) {
                    quote! { None }
                } else {
                    let name = type_name(ok_ty);
                    quote! { Some(#name.to_string()) }
                }
            } else {
                quote! { None }
            };

            quote! {
                {
                    let dummy = <#req_type as Default>::default();
                    crate::custom_requests::CustomMethodSchema {
                        method: agent_client_protocol::JsonRpcMessage::method(&dummy).to_string(),
                        params_schema: #params_expr,
                        params_type_name: #params_name_expr,
                        response_schema: #response_expr,
                        response_type_name: #response_name_expr,
                    }
                }
            }
        })
        .collect();

    // Generate the handle_custom_request method.
    let dispatcher = quote! {
        async fn handle_custom_request(
            &self,
            cx: &agent_client_protocol::ConnectionTo<agent_client_protocol::Client>,
            method: &str,
            params: serde_json::Value,
        ) -> Result<serde_json::Value, agent_client_protocol::Error> {
            #(#arms)*
            Err(agent_client_protocol::Error::method_not_found())
        }
    };

    // Generate the custom_method_schemas method.
    let schemas_fn = quote! {
        pub fn custom_method_schemas(generator: &mut schemars::SchemaGenerator) -> Vec<crate::custom_requests::CustomMethodSchema> {
            vec![
                #(#schema_entries),*
            ]
        }
    };

    // Append the generated methods to the impl block.
    let dispatcher_item: ImplItem =
        syn::parse2(dispatcher).expect("generated dispatcher must parse");
    impl_block.items.push(dispatcher_item);

    let schemas_item: ImplItem = syn::parse2(schemas_fn).expect("generated schemas fn must parse");
    impl_block.items.push(schemas_item);

    TokenStream::from(quote! { #impl_block })
}

struct Route {
    request_type: Type,
    fn_ident: syn::Ident,
    param_type: Option<Type>,
    uses_connection: bool,
    #[allow(dead_code)]
    return_type: Option<Type>,
    ok_type: Option<Type>,
}

fn extract_parameter_types(sig: &syn::Signature) -> Vec<Type> {
    sig.inputs
        .iter()
        .filter_map(|input| match input {
            FnArg::Typed(pat_type) => {
                if matches!(&*pat_type.pat, Pat::Ident(pat) if pat.ident == "self") {
                    None
                } else {
                    Some((*pat_type.ty).clone())
                }
            }
            FnArg::Receiver(_) => None,
        })
        .collect()
}

/// Extract the full return type (e.g. `Result<T, E>`).
fn extract_return_type(sig: &syn::Signature) -> Option<Type> {
    if let ReturnType::Type(_, ty) = &sig.output {
        Some((**ty).clone())
    } else {
        None
    }
}

/// Extract `T` from `Result<T, E>` in the return type.
fn extract_result_ok_type(sig: &syn::Signature) -> Option<Type> {
    let ty = match &sig.output {
        ReturnType::Type(_, ty) => ty,
        _ => return None,
    };

    // Peel through the type to find a path ending in `Result`.
    if let Type::Path(type_path) = ty.as_ref() {
        let last_seg = type_path.path.segments.last()?;
        if last_seg.ident == "Result" {
            if let PathArguments::AngleBracketed(args) = &last_seg.arguments {
                // First generic argument is the Ok type.
                if let Some(GenericArgument::Type(ok_ty)) = args.args.first() {
                    return Some(ok_ty.clone());
                }
            }
        }
    }
    None
}

/// Extract the last segment name from a type path (e.g. `GetSessionRequest` from
/// `crate::custom_requests::GetSessionRequest` or just `GetSessionRequest`).
fn type_name(ty: &Type) -> String {
    if let Type::Path(type_path) = ty {
        if let Some(seg) = type_path.path.segments.last() {
            return seg.ident.to_string();
        }
    }
    quote::quote!(#ty).to_string()
}

/// Check if a type is `serde_json::Value` (matches `Value` or `serde_json::Value`).
fn is_json_value(ty: &Type) -> bool {
    if let Type::Path(type_path) = ty {
        let segments: Vec<_> = type_path
            .path
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect();
        let strs: Vec<&str> = segments.iter().map(|s| s.as_str()).collect();
        matches!(strs.as_slice(), ["serde_json", "Value"] | ["Value"])
    } else {
        false
    }
}

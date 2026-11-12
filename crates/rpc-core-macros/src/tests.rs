use super::*;

#[test]
fn unsupported_contracts_fail_closed() {
    let declarations = [
        quote!(#[rpc(server)] trait Api { #[method(name="x", invented)] fn x(&self) -> RpcResult<()>; }),
        quote!(#[rpc(server, invented)] trait Api { #[method(name="x")] fn x(&self) -> RpcResult<()>; }),
        quote!(#[rpc(server)] trait Api { #[cfg_attr(test, method(name="other"))] #[method(name="x")] fn x(&self) -> RpcResult<()>; }),
        quote!(#[rpc(server)] trait Api { #[method(name="x")] fn x(&self, #[argument(invented="x")] x: u32) -> RpcResult<()>; }),
        quote!(#[rpc(server)] trait Api { #[method(name="x")] fn x(&self) -> CustomResult<()>; }),
        quote!(#[rpc(server)] trait Api<T> { #[method(name="x")] fn x(&self) -> RpcResult<T>; }),
        quote!(#[rpc(server)] trait Api { #[method(name="x", name="y")] fn x(&self) -> RpcResult<()>; }),
        quote!(#[rpc(server)] trait Api { #[subscription(name="x", item=u32)] fn x(&self); }),
        quote!(#[rpc(server)] trait Api { #[subscription(name="subscribe_x")] fn x(&self); }),
        quote!(#[rpc(server)] trait Api { #[method(name="x")] fn x(&self, #[cfg(test)] x: u32) -> RpcResult<()>; }),
    ];
    for declaration in declarations {
        assert!(expand(quote!(owner="tiny", descriptors=descriptors), syn::parse2(declaration).unwrap()).is_err());
    }
}

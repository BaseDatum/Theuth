//! Isolated RPC state for kernel fixtures without session or attachment plugins.

pub(crate) fn new_server(rt: runtime::Runtime) -> rpc_core::Server {
    agent::rpc_forward::install_forward_hooks(agent::rpc_forward::ForwardHooks {
        sessions_running: || false,
        expected: |_| None,
        attachment_name: |_| panic!("kernel fixture has no attachment service"),
        attachments_dir: |_| panic!("kernel fixture has no attachment service"),
        fetch_attachment: |_, _, _, _| panic!("kernel fixture has no attachment service"),
        fetch_contact_file: |_, _, _, _, _| panic!("kernel fixture has no contact files"),
    });
    rpc_core::Server::new(rt)
}

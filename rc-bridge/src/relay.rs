//! 中继传输：按角色连上 rc-server 的 `:21117`（`RCR1` + 双向 mTLS）。
//!
//! 中继是打洞失败时的回退，也是跨网段唯一可用的路。两端各自用服务端下发的一次性令牌
//! 连上中继，中继按 `role`（`0x01` 被控端 / `0x02` 控制端）把两端配对，之后就是一条
//! 全双工字节流，会话层在上面跑 Hello/Welcome 与媒体。

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use rc_client_core::relay_transport::{RelayRole, SessionMtlsBundle, TcpRelayTransport};
use rc_net::Transport;

/// 连上中继并交回一条可用的会话传输。
///
/// @param relay_addr - 服务端下发的中继地址（`host:port`）。
/// @param role - 本端角色。
/// @param token - 一次性 `RCR1` 令牌。
/// @param mtls - 双向 mTLS 材料（含预期对端）。
/// @param timeout - 连接与握手上限。
pub fn connect_relay(
    relay_addr: &str,
    role: RelayRole,
    token: &str,
    mtls: &SessionMtlsBundle,
    timeout: Duration,
) -> Result<Box<dyn Transport>, String> {
    let cfg = mtls.to_relay_mtls_config(None);
    let cancel = Arc::new(AtomicBool::new(false));
    let transport = TcpRelayTransport::connect_mtls_cancellable(
        relay_addr,
        role,
        token,
        timeout,
        cancel,
        &cfg,
    )
    .map_err(|e| format!("中继连接失败（{relay_addr}）：{e}"))?;
    eprintln!(
        "rc-bridge 中继：已连上 {relay_addr}（角色 {role:?}），中继侧对端 {}",
        transport.peer_addr()
    );
    Ok(Box::new(transport))
}

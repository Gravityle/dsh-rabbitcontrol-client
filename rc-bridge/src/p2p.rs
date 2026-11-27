//! P2P 打洞：用信令下发的候选地址打一条直连 QUIC（`RCP2` + BiDi mTLS）。
//!
//! 选路顺序是「局域网候选 → 公网候选」由 `rc-client-core` 的 `parse_candidate_addrs`
//! 决定；打洞成功后拿到的是完整 QUIC 传输（datagram 媒体 + reliable 控制 + urgent 输入），
//! 与中继那条路在会话层看来完全一样。
//!
//! 打洞失败（对端没有候选地址、NAT 打不通、超时）就由调用方退回中继。

use std::sync::atomic::AtomicBool;
use std::time::Duration;

use rc_client_core::p2p::{attempt_p2p_quic_client_mtls, attempt_p2p_quic_host_mtls, P2pCandidates};
use rc_client_core::relay_transport::SessionMtlsBundle;
use rc_net::Transport;

/// 一次打洞的时间预算。
pub struct PunchBudget {
    /// 打洞（UDP 探测对齐）上限。
    pub punch: Duration,
    /// 打洞成功后的 QUIC 握手上限。
    pub quic: Duration,
}

impl Default for PunchBudget {
    fn default() -> Self {
        // 打洞预算要**短**：没有 LAN 候选（例如插件自带的被控端）时打洞基本必失败，
        // 而两端都要"打洞失败 → 转中继"，预算太长会让先转过去的那端在配对前就超时。
        Self { punch: Duration::from_secs(2), quic: Duration::from_secs(3) }
    }
}

/// 控制端打洞。
///
/// @param candidates - 信令下发的候选地址。
/// @param mtls - 双向 mTLS 材料（含预期对端）。
/// @param budget - 时间预算。
pub fn punch_client(
    candidates: &P2pCandidates,
    mtls: &SessionMtlsBundle,
    budget: &PunchBudget,
) -> Result<Box<dyn Transport>, String> {
    let config = mtls.to_quic_mtls_config()?;
    let cancel = AtomicBool::new(false);
    let connection = attempt_p2p_quic_client_mtls(
        candidates,
        budget.punch,
        budget.quic,
        &cancel,
        Some(&config),
    )?;
    eprintln!(
        "rc-bridge P2P：控制端打洞成功，对端 {}（RTT {}µs），mTLS 身份 {}",
        connection.peer_addr,
        connection.rtt_us,
        if connection.peer_mtls_identity().is_some() { "已验证" } else { "缺失" }
    );
    Ok(Box::new(connection))
}

/// 被控端打洞。
///
/// @param candidates - 信令下发的候选地址。
/// @param mtls - 双向 mTLS 材料（含预期对端）。
/// @param budget - 时间预算。
pub fn punch_host(
    candidates: &P2pCandidates,
    mtls: &SessionMtlsBundle,
    budget: &PunchBudget,
) -> Result<Box<dyn Transport>, String> {
    let config = mtls.to_quic_mtls_config()?;
    let cancel = AtomicBool::new(false);
    let connection = attempt_p2p_quic_host_mtls(
        candidates,
        budget.punch,
        budget.quic,
        &cancel,
        Some(&config),
    )?;
    eprintln!(
        "rc-bridge P2P：被控端打洞成功，对端 {}（RTT {}µs），mTLS 身份 {}",
        connection.peer_addr,
        connection.rtt_us,
        if connection.peer_mtls_identity().is_some() { "已验证" } else { "缺失" }
    );
    Ok(Box::new(connection))
}

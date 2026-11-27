# DSH port-hold adapter

Upstream: TokenBoom/rabbitcontrol-core v0.2.5, commit 2911ae17543c1ac6c9a7599f269509a0bd1d9461. AGPL-3.0-only; LICENSE preserved.

Only signal_client visibility is changed: LocalUdpPortHoldGuard, acquire_local_udp_port_for_p2p, P2pHeldTransport, its constructor and peer identity getter are public. Two public method documentation comments are added. No protocol or transport behavior changes.

Core 0.2.5 keeps an unconnected dual-stack RCSP socket bound. External callers of public attempt_p2p_quic_* must release that socket through the same port-hold API used internally by upstream host_bridge/session_bridge. Retain P2pHeldTransport for the entire QUIC lifetime; dropping the hold immediately after dialing would reintroduce port conflicts.

Cargo manifest workspace metadata is materialized and sibling path dependencies use the exact upstream v0.2.5 git source. The bridge patch section replaces this crate globally, including dependencies from macOS, preventing two client-core signal registries.

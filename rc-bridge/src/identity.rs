//! 设备身份：向 rc-server 申请一张 7 天设备证书（mTLS 用）。
//!
//! 信令登记之后，服务端会确认本端的 9 位设备码；拿这个设备码 + `CertManager` 生成的
//! CSR 去 `/api/v1/certs/issue` 换回叶子证书。中继（`RCR1`）与打洞（`RCP2`）都是
//! 双向 mTLS，双方都用这张证书互相验明身份（SAN 里带 user/device）。

use std::path::{Path, PathBuf};

use rc_client_core::auth_client::AuthHttpClient;
use rc_client_core::cert_manager::CertManager;
use rc_client_core::server_config::ServerConfig;

/// 一端的设备身份（内存形态，够构造 `SessionMtlsBundle`）。
pub struct DeviceIdentity {
    /// 叶子证书 PEM。
    pub cert_pem: String,
    /// 与本证书配对的私钥 PEM。
    pub key_pem: String,
    /// Root CA PEM。
    pub ca_pem: String,
    /// 绑定用户 UUID。
    pub user_id: String,
    /// 9 位设备码。
    pub device_id: String,
}

/// 加载或生成本机设备密钥，并申请一张设备证书。
///
/// @param cfg - 服务器配置（host/api 端口、CA、device_key）。
/// @param token - 账号 access token。
/// @param device_id - 信令登记后确认的 9 位设备码。
/// @param dir - 密钥与证书的存放目录（通常放临时目录）。
pub fn provision(
    cfg: &ServerConfig,
    token: &str,
    device_id: &str,
    dir: &Path,
) -> Result<DeviceIdentity, String> {
    let manager = CertManager::load_or_init(PathBuf::from(dir))?;
    let csr_pem = manager.generate_csr_pem(Some("rc-bridge"))?;
    let client = AuthHttpClient::new(
        cfg.server_host.clone(),
        cfg.api_port,
        cfg.use_https,
        cfg.ca_cert_pem.clone(),
    )
    .map_err(|e| format!("AuthClient 构造失败：{e:?}"))?;
    let issued = client
        .issue_device_cert(token, device_id, &csr_pem)
        .map_err(|e| format!("签发设备证书失败：{e:?}"))?;
    eprintln!(
        "rc-bridge 证书：已签发 device_id={} serial={} 指纹={}…",
        issued.device_id,
        issued.serial_hex,
        issued.fingerprint_sha256.chars().take(16).collect::<String>()
    );
    Ok(DeviceIdentity {
        cert_pem: issued.cert_pem,
        key_pem: manager.tls_key_pem()?,
        ca_pem: issued.ca_cert_pem,
        user_id: issued.user_id,
        device_id: issued.device_id,
    })
}

/// Anchor the ticket key in the configured HTTPS endpoint, never in an incoming grant.
pub fn owner_verifier(cfg: &ServerConfig, identity: &DeviceIdentity)
    -> Result<rc_client_core::cert_manager::RcOwnerTicketVerifier, String>
{
    if !cfg.use_https {
        return Err("同账号票据校验需要 HTTPS 信令服务".into());
    }
    let api = AuthHttpClient::new(cfg.server_host.clone(), cfg.api_port, true, Some(identity.ca_pem.clone()))
        .map_err(|e| format!("票据密钥客户端错误：{e:?}"))?;
    let bundle = api.fetch_ca_bundle().map_err(|e| format!("获取服务端票据公钥失败：{e:?}"))?;
    rc_client_core::cert_manager::RcOwnerTicketVerifier::new(
        &bundle.auth_pubkey_hex, identity.user_id.clone(), identity.device_id.clone(),
    )
}

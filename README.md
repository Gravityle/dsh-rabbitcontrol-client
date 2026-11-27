# RabbitControl 远程桌面（dsh-rabbitcontrol-client）

在 DeepSeek Harness 里打开 RabbitControl 被控端的画面，并把键盘与鼠标回传给对端。解码在浏览器侧用 WebCodecs，不需要额外播放器。

## 安装

在 DeepSeek Harness 中把下面这句话发给助手，即可安装固定版本：

```text
请用 plugin_manager 的 install_bundle 安装 https://github.com/Gravityle/dsh-rabbitcontrol-client/releases/download/v0.12.0/dsh-rabbitcontrol-client-0.12.0.tgz
```

安装后打开左侧「远程桌面」，填写自己的 rc-server 地址并登录。包内附带 Apple Silicon Mac 和 Windows x64 边车，这两种平台可将 `bridgePath` 留空。

### 边车版本与构建

本版随包边车使用 RabbitControl Core `v0.2.3`、macOS 平台库 `v0.2.2` 和 Apple Media `v0.2.1`。个人仓库随版本提供 `rc-bridge/` Rust 源码、锁文件和 AGPL-3.0 许可证。进入该目录后，在 Apple Silicon Mac 上运行 `cargo build --locked --release --features real-capture`，在 Windows x64 上运行 `cargo build --locked --release`。发布构建使用 `--remap-path-prefix` 映射本机路径，macOS 产物另做 ad-hoc 签名。

## 入口

| 入口 | 位置 |
|---|---|
| 左侧面板列表的「远程桌面」图标 | 与「插件」「任务」并列；点开在中央区域显示面板 |
| 快捷键 `⌥⌘J`（Windows/Linux 为 `Ctrl+Alt+J`） | 切到同一面板；面板未注册成功时退回右侧标签页 |

## 使用

1. **第一次打开面板会看到「连接 rc-server」设置卡片**：填 rc-server 的 REST 地址（例如 `https://你的服务器:21118`）→ **保存地址** → 用 rc-server 账号**登录**。
   - 地址会写进插件配置（写不进时面板会说明），登录态存进凭据记录，**与「服务器状态」面板共用**；
   - 换服务器会自动清掉旧登录态，需要重新登录；账号首登被要求改密时，面板会提示先去改密。
2. 登录后点「我的设备」中的在线设备。已公布直连地址的设备会依次尝试这些地址，随后尝试信令；无直连地址的设备直接使用信令。自动模式优先打洞，失败回退中继。
3. 在线设备排在前面。对端关闭「允许远程控制」时，该行显示原因并禁用连接；旧服务端省略权限时沿用 Core 的平台默认值。
4. 首次连接使用被控端可提供的最高分辨率。面板顶部显示实测 FPS，旁边的切换框显示当前解码宽高。选其他尺寸会短暂重连同一设备。点击画面右下角的全屏图标进入全屏，再点一次或按 Esc 退出。
5. 需要开机时打开「显示离线设备」。对于同账号下已启用 Wake-on-LAN、且服务端报告支持唤醒的离线设备，可手动发送唤醒请求。设备所在局域网需要可用的在线代理。请求结果以服务端回执为准，设备重新显示在线后即可连接。
6. 点齿轮打开「高级与手动」抽屉，可查看帧率、设置手动地址和键鼠输入，或回放本地 H.264 片段。

## 配置

| 字段 | 默认 | 说明 |
|---|---|---|
| `bridgePath` | 空 | 空 = 用插件自带的边车（按平台选）；也可指定自有路径 |
| `clipPath` | 空 | 只有回放模式用 |
| `serverUrl` | 空 | rc-server 的 REST 地址；**留空（推荐）**时由面板的首次设置卡片填写并写回配置 |
| `insecureTls` | `true` | 接受自签证书 |
| `signalDeviceKey` | `dsh-rabbitcontrol-viewer`（随包配置） | 信令设备 key；停用后请联系管理员 |
| `signalTransport` | `auto` | `auto` / `p2p` / `relay` |

## 让本机变成「可被控」（不用额外脚本）

面板「高级与手动」里有一个 **「作为被控端」** 开关：勾上之后，插件自己拉起边车把本机登记到 rc-server，使用同一账号登录的其他设备即可在设备列表中选择本机。

- macOS 上默认提供**真实屏幕**（ScreenCaptureKit + VideoToolbox，插件自带的边车已带 `real-capture` 特性）；需要给 DSH 应用授予「屏幕录制」权限。
- 随包的 Windows x64 边车用于控制其他设备。要让 Windows 共享真实屏幕，上游已有独立 GUI 的源码，使用 DXGI 和 Media Foundation；本插件的 Windows 边车没有集成该采集后端。Windows 上游主分支已实现 SendInput 键鼠注入和重连进度显示，目前通过源码构建使用。[Windows 上游源码](https://github.com/TokenBoom/rabbitcontrol-win/tree/02046f9a0)
- 设备码与后端标签（如 `sck+videotoolbox`）会显示在开关下面；`hostDeviceKey` 与客户端的 `signalDeviceKey` 分开，避免互相顶掉。

## 两个行为细节

- **断开后可以立即重连。** 两台设备均更新到 v0.11.2 后，控制端会先结束旧会话，被控端会继续等待下一次连接。「作为被控端」可以保持开启。对于每次只服务一个会话的直连程序（如 `rc-cli host`），连接失败或零帧时会在 2 秒后重试同一地址一次。
- **设备显示名自动上报**：被控端上报的 `hostname` 取**宿主机名**（macOS 用「电脑名称」，例如 `mac-mini`），不需要手工设别名；面板优先显示 `alias`，没有就显示 `hostname`。同一台机器跑多个被控端时，可以用角色的后缀区分（例如 `mac-mini · 真桌面`）。

## 前提与限制

- 对端必须是**支持信令的被控端**（`rc-bridge --serve … --signal-*`）或**地址可直达**的被控端（如 macOS 上的 `rc-cli host --listen`）。
- 跨网段且对端位于代理/NAT 之后时，打洞可能"握手成功但媒体不通"；本插件会自动改走中继，所以中继是可靠路径。

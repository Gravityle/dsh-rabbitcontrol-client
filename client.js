/**
 * RabbitControl 客户端 —— Client 半边。
 *
 * 在右侧栏注册 `rabbitcontrol-client` 页类型：从同源流式路由读边车的成帧输出，
 * 用 WebCodecs 解码 Annex-B H.264 并画到 canvas 上。
 *
 * 目前的阶段：只做**观看**（里程碑 3）。键鼠回传尚未接入。
 */
window.__ModuleLoader__.load({
  id: 'dsh-rabbitcontrol-client',
  factory(require) {
    const React = require('react')
    const h = React.createElement
    const { useCallback, useEffect, useRef, useState } = React

    const NS = 'rabbitcontrolClient'
    /** 中央面板与左侧入口共享的面板 id（`main` 槽的 key、`sidebar.panellist` 的 id）。 */
    const PANEL_ID = 'dsh-rabbitcontrol-client'
    /** 中央面板是否注册成功：失败时快捷键退回右侧标签页。 */
    let panelReady = false
    const ID = 'dsh-rabbitcontrol-client'
    const KIND = 'rabbitcontrol-client'
    const RESOLUTIONS = ['3840x2160', '2560x1440', '1920x1080', '1600x900', '1280x720', '854x480']
    const STREAM = '/rabbitcontrol-client/stream'
    const INPUT = '/rabbitcontrol-client/input'
    const DEVICES = '/rabbitcontrol-client/devices'
    const SESSION = '/rabbitcontrol-client/session'
    const SERVER = '/rabbitcontrol-client/server'
    const LOGIN = '/rabbitcontrol-client/login'
    const LOGOUT = '/rabbitcontrol-client/logout'
    const HOST = '/rabbitcontrol-client/host'
    const REPORT = '/rabbitcontrol-client/report'
    const FENCE_401 = 'host-fence-401'
    /** 单帧字节上限（16 MiB）：超过就认为流错位。 */
    const MAX_FRAME_BYTES = 16 * 1024 * 1024
    /** 拆帧缓冲上限（32 MiB）。 */
    const MAX_BUFFER_BYTES = 32 * 1024 * 1024

    const ZH = {
      title: 'RabbitControl 远程桌面',
      shortTitle: '远程桌面',
      description: '连接被控端并在面板里观看远端画面（WebCodecs 解码）。',
      source: '来源',
      modeReplay: '回放测试片段',
      modeConnect: '连接被控端（直连地址）',
      modeSignal: '经服务器（中继）',
      deviceId: '设备码',
      direct: '直连',
      relay: '中继',
      relayHint: '跨网段用它：两端各自连中继，需要 rc-server 可达且已在「服务器状态」面板登录。',
      retryAddress: '这个地址没通，自动换该设备的下一个地址重试…',
      retryRelay: '直连地址都没通，自动改走中继重试…',
      address: '被控端地址',
      clip: '片段路径',
      fps: '帧率',
      connect: '连接',
      disconnect: '断开',
      connecting: '连接中…',
      watching: '观看中',
      idle: '未连接',
      frames: '已收帧',
      decoded: '已解码',
      dropped: '丢弃',
      decodeErrors: '解码错误',
      resolution: '分辨率',
      decodedSize: '实际解码尺寸',
      reconnectHint: '更改后将重新连接当前设备。',
      reconnecting: '正在按新分辨率重连…',
      decoder: '解码器',
      waitingKey: '等待关键帧…',
      noWebCodecs: '当前浏览器不支持 WebCodecs（VideoDecoder），无法解码画面。',
      hint: '点击画面后用键盘与鼠标操作远端；勾掉「发送键鼠」即停止回传。',
      inputOn: '发送键鼠',
      inputSent: '已发事件',
      inputNone: '尚未连接会话，输入未发送。',
      inputDropped: '不支持的按键（未计入）',
      focusHint: '点击画面操作远端，按 Shift+Esc 返回面板',
      needHost: '需要先在别处把被控端跑起来（例如 rc-cli host --listen 127.0.0.1:9901 --backend real）。',
      myDevices: '我的设备',
      refresh: '刷新',
      noDevices: 'rc-server 上还没有已注册设备。',
      needSignIn: '设备列表需要登录 rc-server：请先在「RabbitControl 服务器状态」面板登录。',
      deviceOffline: '离线',
      deviceNoAddress: '未公布地址',
      busy: '正在连接',
      manual: '手动地址（直连）',
      advanced: '高级与手动',
      connectHint: '选择允许远控的在线设备：有地址会先直连，失败自动改走中继；没有地址直接走中继。',
      closeDrawer: '关闭',
      controlDisabled: '对端已关闭远程控制',
      thisMachine: '本机',
      selfHint: '这就是当前这台电脑：不需要连接它（别的电脑会在各自列表里连它）。',
      fullscreen: '全屏画面',
      exitFullscreen: '退出全屏',
      routeDirect: '直连',
      routeP2P: 'P2P 打洞',
      routeRelay: '中继',
      routeHint: '本次会话实际走的链路：直连（局域网/地址）、P2P 打洞（NAT 打洞成功）、中继（经服务器转发）',
      routeDirectFirst: '直连优先',
      routeRelayFirst: '经中继',
      showOffline: '显示离线设备',
      offlineHidden: '台离线设备已隐藏',
      hostEnable: '作为被控端（让别人看本机屏幕）',
      hostRunning: '被控端运行中',
      hostStopped: '被控端未运行',
      hostDevice: '设备码',
      hostNeedLogin: '需要先登录 rc-server 才能作为被控端登记。',
      windowsHostHint: 'Windows 随包边车可控制其他设备。要共享本机屏幕，请登录独立的 RabbitControl Windows GUI 并打开「本机被控」。该版本支持采集，键鼠注入尚未实现。',
      hostHint: '打开后本机会登记到 rc-server 并公示屏幕；真实屏幕采集需要所选边车支持。',
      hostInjectReal: '输入注入=真实（CGEvent）：对端可以操作本机的鼠标键盘。',
      hostInjectBlocked: '输入注入被系统拦下：对端只能看、不能操作。请到「系统设置 → 隐私与安全性 → 辅助功能」勾选 DeepSeek Harness，然后完全退出并重开 DSH。',
      setupTitle: '连接 rc-server',
      setupHint: '填 rc-server 的 REST 地址（例如 https://你的服务器:21118），保存后再用账号登录；地址会写进插件配置，下次不用再填。',
      server: '服务器',
      serverSaved: '地址已保存，请登录。',
      email: '邮箱',
      password: '口令',
      saveServer: '保存地址',
      signIn: '登录',
      signOut: '退出登录',
      signedInAs: '已登录',
      notConfigured: '未配置服务器',
      needServer: '请先填 rc-server 地址。',
      needCredentials: '请填邮箱与口令。',
      mustChangePassword: '这个账号需要先改口令（在「服务器状态」面板或 rc-server 控制台改完再登录）。',
      pickDevice: '从左侧选一台设备开始',
      panel: '远程桌面',
      sessionEnded: '会话已结束',
      undecodable: '已收到帧但一帧都没解出来：这通常是假后端的合成数据，不是真 H.264。用真后端或回放测试片段再看。',
      decodeFailed: '解码失败',
      fenceError: '面板接口拒绝了这次请求（浏览器身份未通过）。刷新页面后重试。',
      streamError: '帧流中断',
    }
    const EN = {
      title: 'RabbitControl remote screen',
      shortTitle: 'Remote screen',
      description: 'Connect to a controlled device and watch its screen (WebCodecs decode).',
      source: 'Source',
      modeReplay: 'Replay test clip',
      modeConnect: 'Connect to device (direct address)',
      modeSignal: 'Via server (relay)',
      deviceId: 'Device code',
      direct: 'Direct',
      relay: 'Relay',
      relayHint: 'For devices across networks: both ends connect to the relay; needs a reachable rc-server and a sign-in on the server-status panel.',
      retryAddress: 'That address did not work; trying the next advertised address…',
      retryRelay: 'No direct address worked; retrying through the relay…',
      address: 'Device address',
      clip: 'Clip path',
      fps: 'Frame rate',
      connect: 'Connect',
      disconnect: 'Disconnect',
      connecting: 'Connecting…',
      watching: 'Watching',
      idle: 'Not connected',
      frames: 'Received',
      decoded: 'Decoded',
      dropped: 'Dropped',
      decodeErrors: 'Decode errors',
      resolution: 'Resolution',
      decodedSize: 'Decoded size',
      reconnectHint: 'Changing resolution reconnects to this device.',
      reconnecting: 'Reconnecting at the new resolution…',
      decoder: 'Decoder',
      waitingKey: 'Waiting for a keyframe…',
      noWebCodecs: 'This browser has no WebCodecs (VideoDecoder); the picture cannot be decoded.',
      hint: 'Click the picture to send keyboard and mouse to the device; clear “Send input” to stop.',
      inputOn: 'Send input',
      inputSent: 'Events sent',
      inputNone: 'No session yet; input was not sent.',
      inputDropped: 'Unsupported keys (not sent)',
      focusHint: 'Click the picture to control the remote device. Press Shift+Esc to return to the panel.',
      needHost: 'Start the controlled side first, e.g. rc-cli host --listen 127.0.0.1:9901 --backend real.',
      myDevices: 'My devices',
      refresh: 'Refresh',
      noDevices: 'No registered device on rc-server yet.',
      needSignIn: 'The device list needs an rc-server sign-in: use the “RabbitControl server status” panel first.',
      deviceOffline: 'offline',
      deviceNoAddress: 'no address',
      busy: 'connecting',
      manual: 'Manual address (direct)',
      advanced: 'Advanced and manual',
      connectHint: 'Select an online device that allows remote control: it tries a direct address first and falls back to the relay; devices without an address go straight to the relay.',
      closeDrawer: 'Close',
      controlDisabled: 'Remote control is disabled on this device',
      thisMachine: 'This machine',
      selfHint: 'This is the machine you are using: there is no need to connect to it (other machines connect to it from their own lists).',
      fullscreen: 'Full screen',
      exitFullscreen: 'Exit full screen',
      routeDirect: 'direct',
      routeP2P: 'P2P',
      routeRelay: 'relay',
      routeHint: 'The route this session actually uses: direct (LAN/address), P2P (NAT punch succeeded), or relay (through the server).',
      routeDirectFirst: 'direct first',
      routeRelayFirst: 'via relay',
      showOffline: 'Show offline devices',
      offlineHidden: 'offline devices hidden',
      hostEnable: 'Act as a controlled device (let others view this screen)',
      hostRunning: 'Controlled side is running',
      hostStopped: 'Controlled side is stopped',
      hostDevice: 'Device',
      hostNeedLogin: 'Sign in to rc-server before this machine can register as a controlled device.',
      windowsHostHint: 'The bundled Windows bridge controls other devices. To share this screen, sign in to the separate RabbitControl Windows GUI and turn on “本机被控”. Its current source supports capture; keyboard and mouse injection are pending.',
      hostHint: 'When enabled this machine registers with rc-server and offers its screen. Sharing a real screen requires capture support in the selected bridge.',
      hostInjectReal: 'Input injection is live (CGEvent): the peer can drive this machine.',
      hostInjectBlocked: 'Input injection is blocked by macOS: the peer can watch but not control. Grant Accessibility to DeepSeek Harness in System Settings → Privacy & Security → Accessibility, then fully quit and reopen DSH.',
      setupTitle: 'Connect to rc-server',
      setupHint: 'Enter the rc-server REST address (for example https://your-server:21118), save it, then sign in. The address is stored in the plugin configuration, so you only do this once.',
      server: 'Server',
      serverSaved: 'Address saved. Please sign in.',
      email: 'Email',
      password: 'Password',
      saveServer: 'Save address',
      signIn: 'Sign in',
      signOut: 'Sign out',
      signedInAs: 'Signed in',
      notConfigured: 'No server configured',
      needServer: 'Enter the rc-server address first.',
      needCredentials: 'Enter both email and password.',
      mustChangePassword: 'This account must change its password first (do that in the server status panel or the rc-server console, then sign in).',
      pickDevice: 'Pick a device on the left to start',
      panel: 'Remote screen',
      sessionEnded: 'Session ended',
      undecodable: 'Frames arrive but none decode: this is usually fake-backend synthetic data, not real H.264. Use the real backend or replay the test clip.',
      decodeFailed: 'Decode failed',
      fenceError: 'The panel endpoint rejected this request (browser identity). Refresh the page and retry.',
      streamError: 'Frame stream ended',
    }

    const CSS = [
      /* 页面骨架：铺满槽位；颜色一律走主题令牌（浅色/深色自动跟随），字面颜色只用于画面本身。 */
      '.rcc-root{position:relative;display:flex;flex-direction:column;height:100%;min-height:0;box-sizing:border-box;container-type:size;container-name:rcc;background:var(--dsw-alias-bg-base);color:var(--dsw-alias-label-primary);font-size:var(--dsw-content-font-size-secondary,13px)}',
      /* 页头：与宿主页头同规格（14/20 标题、13/20 次级、细分隔线、侧栏底色）。 */
      '.rcc-head{flex:none;display:flex;flex-wrap:wrap;align-items:center;gap:8px 12px;padding:12px 16px;border-bottom:1px solid var(--dsw-alias-border-l1);background:var(--dsw-specific-sidebar-fill,var(--dsw-alias-bg-layer-1))}',
      '.rcc-head-main{display:flex;align-items:center;gap:10px;min-width:0}',
      '.rcc-head-text{display:flex;flex-direction:column;min-width:0}',
      '.rcc-title{font-size:14px;line-height:20px;font-weight:500;white-space:nowrap}',
      '.rcc-sub{font-size:13px;line-height:20px;color:var(--dsw-alias-label-secondary);overflow:hidden;text-overflow:ellipsis;white-space:nowrap}',
      '.rcc-head-actions{display:flex;flex-wrap:wrap;align-items:center;gap:8px;margin-left:auto;max-width:100%}',
      '.rcc-resolution{display:flex;flex-wrap:wrap;align-items:center;gap:6px;max-width:100%}',
      '.rcc-dot{flex:none;width:8px;height:8px;border-radius:50%;background:var(--dsw-alias-state-idle-primary)}',
      '.rcc-dot-live{background:var(--dsw-alias-state-success-primary)}',
      '.rcc-dot-warn{background:var(--dsw-alias-state-warn-primary)}',
      '.rcc-chip{display:inline-flex;align-items:center;gap:6px;padding:2px 8px;border:1px solid var(--dsw-alias-border-l1);border-radius:999px;background:var(--dsw-alias-bg-layer-2);color:var(--dsw-alias-label-secondary);font-size:12px;line-height:18px;font-variant-numeric:tabular-nums;white-space:nowrap}',
      /* 控件：高度 28、圆角 6，与宿主控件一致；焦点环沿用宿主令牌。 */
      '.rcc-btn{flex:none;display:inline-flex;align-items:center;justify-content:center;gap:6px;height:28px;padding:0 12px;border:1px solid var(--dsw-alias-border-l1);border-radius:6px;background:var(--dsw-alias-bg-layer-1);color:var(--dsw-alias-label-primary);font:inherit;font-size:13px;line-height:20px;cursor:pointer}',
      '.rcc-btn:disabled{color:var(--dsw-alias-state-idle-primary);cursor:default}',
      '.rcc-btn-primary{border-color:transparent;background:var(--dsw-alias-brand-primary);color:var(--dsw-alias-bg-base)}',
      '.rcc-icon-btn{flex:none;display:inline-flex;align-items:center;justify-content:center;width:28px;height:28px;padding:0;border:1px solid var(--dsw-alias-border-l1);border-radius:6px;background:var(--dsw-alias-bg-layer-1);color:var(--dsw-alias-label-secondary);cursor:pointer;font:inherit;font-size:14px;line-height:1}',
      '.rcc-btn:focus-visible,.rcc-icon-btn:focus-visible,.rcc-input:focus-visible,.rcc-select:focus-visible,.rcc-advanced>summary:focus-visible{outline:var(--dsw-focus-ring-width,2px) solid var(--dsw-focus-ring-color,var(--dsw-alias-brand-primary));outline-offset:1px}',
      '@media (hover:hover){.rcc-row:not(.rcc-row-self):hover{background:var(--dsw-alias-bg-layer-2)}.rcc-advanced>summary:hover{background:var(--dsw-alias-bg-layer-2)}.rcc-btn:hover:not(:disabled){border-color:var(--dsw-alias-border-l2)}.rcc-icon-btn:hover{border-color:var(--dsw-alias-border-l2);color:var(--dsw-alias-label-primary)}.rcc-btn-primary:hover:not(:disabled){filter:brightness(1.06)}.rcc-row:hover{background:var(--dsw-alias-bg-layer-2)}}',
      /* 两栏：左设备列表（管理列表式行）+ 右画面舞台；窄容器自动上下堆叠。 */
      '.rcc-body{flex:1 1 auto;display:flex;min-height:0}',
      '.rcc-list-pane{flex:0 0 240px;display:flex;flex-direction:column;min-height:0;border-right:1px solid var(--dsw-alias-border-l1);background:var(--dsw-alias-bg-layer-1)}',
      '.rcc-pane-head{flex:none;display:flex;align-items:center;gap:8px;padding:10px 12px}',
      '.rcc-pane-title{font-size:13px;line-height:20px;font-weight:500}',
      '.rcc-count{padding:0 6px;border-radius:999px;background:var(--dsw-alias-bg-layer-2);color:var(--dsw-alias-label-secondary);font-size:12px;line-height:18px;font-variant-numeric:tabular-nums}',
      '.rcc-spacer{flex:1 1 auto}',
      '.rcc-rows{flex:1 1 auto;min-height:0;overflow-y:auto;margin:0;padding:0 6px 8px;list-style:none}',
      '.rcc-row{display:flex;align-items:center;gap:10px;padding:12px;border-radius:6px;cursor:pointer}',
      '.rcc-row[aria-disabled="true"]{cursor:default;opacity:.6}',
      '.rcc-row:focus-visible{outline:var(--dsw-focus-ring-width,2px) solid var(--dsw-focus-ring-color,var(--dsw-alias-brand-primary));outline-offset:1px}',
      '.rcc-row-go{flex:none;color:var(--dsw-alias-label-secondary);font-size:16px;line-height:1}',
      '.rcc-row-self{cursor:default;background:var(--dsw-alias-bg-layer-2);opacity:.75}',
      '.rcc-row-self .rcc-row-title{font-weight:500;color:var(--dsw-alias-label-secondary)}',
      '.rcc-self-chip{display:inline-flex;align-items:center;margin-left:6px;padding:0 6px;border:1px solid var(--dsw-alias-border-l1);border-radius:999px;font-size:11px;line-height:16px;font-weight:400;color:var(--dsw-alias-label-secondary)}',
      '.rcc-row-active{background:var(--dsw-alias-bg-layer-2);box-shadow:inset 2px 0 0 var(--dsw-alias-brand-primary)}',
      '.rcc-row-main{flex:1 1 auto;min-width:0}',
      '.rcc-row-title{display:block;font-size:13px;line-height:20px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}',
      '.rcc-row-meta{display:block;font-size:12px;line-height:18px;color:var(--dsw-alias-label-secondary);overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-variant-numeric:tabular-nums}',
      '.rcc-setup{display:flex;flex-direction:column;gap:12px;width:100%;max-width:520px;margin:0 auto;padding:20px;border:1px solid var(--dsw-alias-border-l1);border-radius:10px;background:var(--dsw-alias-bg-layer-1)}',
      '.rcc-setup-title{font-size:14px;line-height:20px;font-weight:500}',
      '.rcc-pane-hint{padding:0 12px 8px}',
      // 左侧抽屉：遮罩 + 从左边滑出；关闭时保持挂载但不可见、不可点。
      '.rcc-drawer-layer{position:absolute;inset:0;z-index:20;display:flex;visibility:hidden}',
      '.rcc-drawer-open{visibility:visible}',
      '.rcc-scrim{position:absolute;inset:0;background:rgba(0,0,0,.55);opacity:0;transition:opacity .18s ease}',
      '.rcc-drawer-open .rcc-scrim{opacity:1}',
      '.rcc-drawer{position:relative;display:flex;flex-direction:column;width:min(360px,88%);height:100%;background:var(--dsw-alias-bg-base);border-right:1px solid var(--dsw-alias-border-l1);transform:translateX(-100%);transition:transform .18s ease;overflow:hidden}',
      '.rcc-drawer-open .rcc-drawer{transform:none}',
      '.rcc-drawer-head{flex:none;display:flex;align-items:center;gap:8px;padding:12px 16px;border-bottom:1px solid var(--dsw-alias-border-l1)}',
      '.rcc-drawer-title{flex:1 1 auto;font-size:14px;line-height:20px;font-weight:500}',
      '.rcc-drawer-body{flex:1 1 auto;min-height:0;display:flex;flex-direction:column;gap:12px;padding:12px 16px 16px;overflow-y:auto}',
      '.rcc-empty{margin:0;padding:4px 12px 16px;font-size:13px;line-height:20px;color:var(--dsw-alias-label-secondary)}',
      '.rcc-stage-pane{flex:1 1 auto;display:flex;flex-direction:column;gap:12px;min-width:0;min-height:0;padding:12px 16px 16px;overflow:hidden}',
      /* 画面容器：16:9、黑底（画面本体用字面黑是合理的）、圆角边框与宿主卡片一致。 */
      '.rcc-stage{flex:1 1 auto;display:flex;align-items:center;justify-content:center;min-width:0;min-height:0}',
      // 固定 16:9 居中：宽度吃满可用宽度，高度由比例决定；超出可用高度时整体按比例收进（画布用 contain，不会变形）。
      '.rcc-frame{position:relative;flex:0 1 auto;aspect-ratio:16/9;width:100%;max-width:100%;max-height:100%;display:flex;align-items:center;justify-content:center;border:1px solid var(--dsw-alias-border-l1);border-radius:8px;background:#000;overflow:hidden}',
      '.rcc-frame-idle{display:none}',
      '.rcc-standby{display:flex;flex-direction:column;align-items:center;justify-content:center;gap:8px;flex:0 1 auto;width:100%;max-width:560px;min-height:0;padding:40px 24px;border:1px dashed var(--dsw-alias-border-l2);border-radius:10px;background:var(--dsw-alias-bg-layer-1);text-align:center}',
      '.rcc-standby-mark{width:32px;height:32px;border-radius:8px;border:1px solid var(--dsw-alias-border-l1);background:var(--dsw-alias-bg-layer-2)}',
      '.rcc-standby-title{font-size:14px;line-height:20px;font-weight:500;color:var(--dsw-alias-label-primary)}',
      '.rcc-canvas{display:block;width:100%;height:100%;object-fit:contain;outline:none}',
      '.rcc-canvas:focus-visible{outline:var(--dsw-focus-ring-width,2px) solid var(--dsw-focus-ring-color,var(--dsw-alias-brand-primary));outline-offset:-2px}',
      '.rcc-placeholder{position:absolute;inset:0;display:flex;flex-direction:column;align-items:center;justify-content:center;gap:6px;padding:24px;text-align:center;color:var(--dsw-alias-label-secondary);font-size:13px;line-height:20px}',
      '.rcc-fps{white-space:nowrap;font-variant-numeric:tabular-nums}',
      '.rcc-frame .rcc-fullscreen{position:absolute;right:8px;bottom:8px;width:36px;height:36px;display:grid;place-items:center;background:var(--dsw-alias-bg-layer-1);color:var(--dsw-alias-label-primary)}',
      '.rcc-overlay{position:absolute;top:8px;right:8px;left:8px;display:flex;justify-content:flex-end;flex-wrap:wrap;gap:4px;pointer-events:none}',
      '.rcc-overlay .rcc-chip{white-space:normal;border-color:transparent;background:rgba(0,0,0,.55);color:#fff}',
      '.rcc-status{flex:none;display:flex;align-items:center;gap:8px;font-size:13px;line-height:20px;color:var(--dsw-alias-label-secondary)}',
      '.rcc-message{flex:none;margin:0;padding:8px 10px;border-radius:8px;font-size:13px;line-height:20px;word-break:break-word}',
      '.rcc-error{border:1px solid var(--dsw-alias-state-error-primary);background:var(--dsw-alias-bg-layer-1);color:var(--dsw-alias-state-error-primary)}',
      '.rcc-notice{border:1px solid var(--dsw-alias-border-l1);background:var(--dsw-alias-bg-layer-1);color:var(--dsw-alias-label-secondary)}',
      '.rcc-metrics{display:grid;grid-template-columns:repeat(auto-fill,minmax(104px,1fr));gap:8px}',
      '.rcc-metric{border:1px solid var(--dsw-alias-border-l1);border-radius:8px;background:var(--dsw-alias-bg-layer-1);padding:8px 10px}',
      '.rcc-metric-value{font-size:16px;line-height:22px;font-variant-numeric:tabular-nums}',
      '.rcc-metric-label{margin-top:2px;font-size:12px;line-height:18px;color:var(--dsw-alias-label-secondary)}',
      /* 高级区折叠：默认不抢注意力。 */
      '.rcc-host{display:flex;flex-direction:column;gap:8px;padding:10px;border:1px solid var(--dsw-alias-border-l1);border-radius:8px;background:var(--dsw-alias-bg-layer-2)}',
      '.rcc-field{display:flex;align-items:center;gap:8px}',
      '.rcc-label{flex:0 0 84px;font-size:13px;line-height:20px;color:var(--dsw-alias-label-secondary)}',
      '.rcc-input,.rcc-select{flex:1 1 auto;min-width:0;box-sizing:border-box;height:28px;padding:0 8px;border:1px solid var(--dsw-alias-border-l1);border-radius:6px;background:var(--dsw-alias-bg-base);color:var(--dsw-alias-label-primary);font:inherit;font-size:13px}',
      '@media (pointer:coarse){.rcc-input,.rcc-select{height:34px;font-size:16px}}',
      '.rcc-check{display:flex;align-items:center;gap:8px;font-size:13px;line-height:20px}',
      '.rcc-hint{margin:0;font-size:12px;line-height:18px;color:var(--dsw-alias-label-secondary)}',
      '.rcc-fill .rcc-list-pane,.rcc-fill .rcc-pane-hint{display:none}',
      '.rcc-fill .rcc-stage-pane{padding:0}',
      '.rcc-fill .rcc-status,.rcc-fill .rcc-message{padding:0 16px}',
      '.rcc-frame:fullscreen{aspect-ratio:auto;width:100%;height:100%;max-height:none;border:0;border-radius:0}',
      // ── 4:3 档位（DSH 窗口常见比例）：左栏横过来放到顶部，画面拿回全部宽度 ──
      '@container rcc (max-aspect-ratio: 16/10){.rcc-body{flex-direction:column}.rcc-list-pane{flex:0 0 auto;max-height:none;border-right:0;border-bottom:1px solid var(--dsw-alias-border-l1)}.rcc-pane-head{padding:8px 12px 0}.rcc-pane-hint{display:none}.rcc-rows{display:flex;gap:8px;overflow-x:auto;overflow-y:hidden;padding:8px 12px 12px}.rcc-row{flex:0 0 auto;width:196px;padding:8px 10px}.rcc-row-meta{display:none}.rcc-empty{padding:0 12px 12px}}',
      '@media (max-width:860px){.rcc-stage-pane{padding:10px 12px 12px}.rcc-stage{min-height:min(44vh,320px)}.rcc-standby{padding:24px 16px}}',
    ].join('')

    /** 面板内联样式。 */
    function Styles() {
      return h('style', null, CSS)
    }

    /**
     * 一行设备的可交互状态。
     *
     * **本机自己那条不可点**：连自己既没意义，也会和本机被控端抢同一条会话。
     * @param device - 设备列表里的一项。
     * @param hostDeviceId - 本机被控端的设备码（未知时传空串）。
     * @param running - 当前是否已有会话在跑。
     * @returns `{ isSelf, connectable }`。
     */
    function deviceRowState(device, hostDeviceId, running) {
      const deviceId = String(device?.deviceId ?? '')
      const isSelf = hostDeviceId !== '' && deviceId === hostDeviceId
      const connectable = device?.online === true && device.allowRemoteControl !== false && running !== true && !isSelf
      return { isSelf, connectable }
    }

    /**
     * 给一台设备挑「第一次尝试」：公布了地址就先直连，没有地址就直接走中继。
     *
     * 失败后的降级由 [`nextAttempt`] 负责（换地址 → 中继），所以用户只需点一下设备行。
     * @param device - 设备列表里的一项。
     * @returns 可直接交给 connect 的目标。
     */
    function firstAttempt(device) {
      const addresses = Array.isArray(device?.lanAddrs)
        ? device.lanAddrs.filter((item) => typeof item === 'string' && item !== '')
        : []
      const deviceId = String(device?.deviceId ?? '')
      if (addresses.length > 0) return { address: addresses[0], addresses, deviceId }
      return { signalDevice: deviceId }
    }

    /**
     * 决定「这次没成」之后要不要自动再试一次。
     *
     * 顺序：先把该设备公布的**其它地址**都试一遍（例如 tailnet 地址不通时退到局域网地址），
     * 都试完还有设备码，就降级到**中继**（信令）再试一次。
     *
     * @param current - `{ mode, address, deviceId, addresses, tried }`，`tried` 含刚用过的那个。
     * @returns 下一次尝试（可直接交给 connect）或 `null`（放弃，交给调用方报错）。
     */
    function nextAttempt(current) {
      const tried = Array.isArray(current.tried) ? current.tried : []
      if (current.mode !== 'connect') return null
      const candidates = Array.isArray(current.addresses) ? current.addresses : []
      const next = candidates.find((item) => typeof item === 'string' && item !== '' && !tried.includes(item))
      if (next !== undefined) return { address: next, viaRelay: false }
      if (typeof current.deviceId === 'string' && current.deviceId !== '') {
        return { signalDevice: current.deviceId, viaRelay: true }
      }
      return null
    }

    /**
     * 面板图标（与包内 icon.svg 同形）。
     * @param props - 左侧面板图标槽传入 `{ size, active }`；guide 卡片则不给。
     * @returns 图标元素。
     */
    function PanelIcon(props) {
      const size = typeof props?.size === 'number' ? props.size : 36
      return h('svg', { viewBox: '0 0 36 36', width: size, height: size, 'aria-hidden': true, focusable: false },
        h('rect', { x: 4.5, y: 8, width: 27, height: 17, rx: 2.5, fill: 'none', stroke: 'currentColor', strokeWidth: 2 }),
        h('path', { d: 'M12 21.5l4.5-5.5 3 3.5 2.5-3 4 5z', fill: 'currentColor' }),
        h('path', { d: 'M13 29h10', fill: 'none', stroke: 'currentColor', strokeWidth: 2, strokeLinecap: 'round' }),
        h('path', { d: 'M18 25v4', fill: 'none', stroke: 'currentColor', strokeWidth: 2, strokeLinecap: 'round' }))
    }

    /**
     * 把注册/渲染错误同时打到控制台与磁盘：没有浏览器控制时也能定位。
     * @param stage - 出错阶段。
     * @param error - 捕获到的错误。
     */
    function reportFailure(stage, error) {
      const message = error instanceof Error ? `${error.message}\n${error.stack ?? ''}` : String(error)
      console.error(`[rabbitcontrol-client] ${stage} failed:`, error)
      try {
        void fetch(REPORT, { method: 'POST', headers: { 'content-type': 'text/plain' }, body: `${stage}: ${message}` })
      } catch {
        // 上报也失败就只剩控制台
      }
    }

    /**
     * 单个注册点兜底：Cordis 会回滚抛错 fiber，一个失败不该让整个面板消失。
     * @param stage - 阶段名。
     * @param register - 真正做注册的函数。
     */
    function guard(stage, register) {
      try {
        register()
      } catch (error) {
        reportFailure(stage, error)
      }
    }

    /**
     * 从 Annex-B 码流里读出 SPS，拼出 WebCodecs 需要的 codec 字符串。
     * @param bytes - 一帧的 Annex-B 数据。
     * @returns `avc1.PPCCLL`，找不到 SPS 时为 undefined。
     */
    function codecFromAnnexB(bytes) {
      for (let i = 0; i + 6 < bytes.length; i += 1) {
        if (bytes[i] === 0 && bytes[i + 1] === 0 && bytes[i + 2] === 1 && (bytes[i + 3] & 0x1f) === 7) {
          const hex = (value) => value.toString(16).padStart(2, '0').toUpperCase()
          return `avc1.${hex(bytes[i + 4])}${hex(bytes[i + 5])}${hex(bytes[i + 6])}`
        }
      }
      return undefined
    }

    /**
     * 一帧里有没有 IDR slice（NAL 5）。
     * @param bytes - Annex-B 数据。
     * @returns 是否关键帧。
     */
    function hasIdr(bytes) {
      for (let i = 0; i + 3 < bytes.length; i += 1) {
        if (bytes[i] === 0 && bytes[i + 1] === 0 && bytes[i + 2] === 1 && (bytes[i + 3] & 0x1f) === 5) return true
      }
      return false
    }

    // Some H.264 encoders deliver parameter sets before the first IDR.
    function startupFrame(bytes, sets) {
      let start = -1
      let type = 0
      let hasSps = false
      let hasPps = false
      function remember(end) {
        if (start < 0 || end - start > 65536) return
        if (type === 7) sets.sps = bytes.slice(start, end)
        if (type === 8) sets.pps = bytes.slice(start, end)
      }
      for (let i = 0; i + 3 < bytes.length; i++) {
        if (bytes[i] || bytes[i + 1] || bytes[i + 2] !== 1) continue
        const prefix = i > 0 && bytes[i - 1] === 0 ? i - 1 : i
        remember(prefix)
        start = prefix
        type = bytes[i + 3] & 31
        hasSps ||= type === 7
        hasPps ||= type === 8
        i += 2
      }
      remember(bytes.length)
      if (!hasIdr(bytes)) return null
      if (hasSps && hasPps) return bytes
      if (!sets.sps || !sets.pps) return null
      const length = sets.sps.length + sets.pps.length + bytes.length
      if (length > MAX_FRAME_BYTES) return null
      const frame = new Uint8Array(length)
      frame.set(sets.sps)
      frame.set(sets.pps, sets.sps.length)
      frame.set(bytes, sets.sps.length + sets.pps.length)
      return frame
    }

    /**
     * DOM `KeyboardEvent.code` → PS/2 Set 1 扫描码。
     *
     * 远端按下的是扫描码而不是虚拟键码（跨布局不乱）；E0 前缀键写成 `0xE0xx`。
     * 表里没有的键会被丢弃并计入「丢弃」，不做猜测。
     */
    const SCANCODES = {
      Escape: 0x01, Digit1: 0x02, Digit2: 0x03, Digit3: 0x04, Digit4: 0x05,
      Digit5: 0x06, Digit6: 0x07, Digit7: 0x08, Digit8: 0x09, Digit9: 0x0a,
      Digit0: 0x0b, Minus: 0x0c, Equal: 0x0d, Backspace: 0x0e, Tab: 0x0f,
      KeyQ: 0x10, KeyW: 0x11, KeyE: 0x12, KeyR: 0x13, KeyT: 0x14, KeyY: 0x15,
      KeyU: 0x16, KeyI: 0x17, KeyO: 0x18, KeyP: 0x19, BracketLeft: 0x1a,
      BracketRight: 0x1b, Enter: 0x1c, ControlLeft: 0x1d, KeyA: 0x1e, KeyS: 0x1f,
      KeyD: 0x20, KeyF: 0x21, KeyG: 0x22, KeyH: 0x23, KeyJ: 0x24, KeyK: 0x25,
      KeyL: 0x26, Semicolon: 0x27, Quote: 0x28, Backquote: 0x29, ShiftLeft: 0x2a,
      Backslash: 0x2b, KeyZ: 0x2c, KeyX: 0x2d, KeyC: 0x2e, KeyV: 0x2f, KeyB: 0x30,
      KeyN: 0x31, KeyM: 0x32, Comma: 0x33, Period: 0x34, Slash: 0x35, ShiftRight: 0x36,
      NumpadMultiply: 0x37, AltLeft: 0x38, Space: 0x39, CapsLock: 0x3a,
      F1: 0x3b, F2: 0x3c, F3: 0x3d, F4: 0x3e, F5: 0x3f, F6: 0x40, F7: 0x41,
      F8: 0x42, F9: 0x43, F10: 0x44, NumLock: 0x45, ScrollLock: 0x46,
      Numpad7: 0x47, Numpad8: 0x48, Numpad9: 0x49, NumpadSubtract: 0x4a,
      Numpad4: 0x4b, Numpad5: 0x4c, Numpad6: 0x4d, NumpadAdd: 0x4e,
      Numpad1: 0x4f, Numpad2: 0x50, Numpad3: 0x51, Numpad0: 0x52,
      NumpadDecimal: 0x53, F11: 0x57, F12: 0x58,
      NumpadEnter: 0xe01c, ControlRight: 0xe01d, NumpadDivide: 0xe035,
      AltRight: 0xe038, Home: 0xe047, ArrowUp: 0xe048, PageUp: 0xe049,
      ArrowLeft: 0xe04b, ArrowRight: 0xe04d, End: 0xe04f, ArrowDown: 0xe050,
      PageDown: 0xe051, Insert: 0xe052, Delete: 0xe053, MetaLeft: 0xe05b,
      MetaRight: 0xe05c, ContextMenu: 0xe05d,
    }

    /** 会被记进修饰键快照的键码。 */
    const MODIFIER_CODES = new Set(['ControlLeft', 'ShiftLeft', 'AltLeft', 'MetaLeft',
      'ControlRight', 'ShiftRight', 'AltRight', 'MetaRight'])

    /**
     * 由按下的修饰键集合算出快照位（位定义与 rc-input 的 `modifier_bits` 一致）。
     * @param pressed - 当前按住的修饰键码集合。
     * @returns 快照字节。
     */
    function modifierBits(pressed) {
      let bits = 0
      if (pressed.has('ControlLeft')) bits |= 0x01
      if (pressed.has('ShiftLeft')) bits |= 0x02
      if (pressed.has('AltLeft')) bits |= 0x04
      if (pressed.has('MetaLeft')) bits |= 0x08
      if (pressed.has('ControlRight')) bits |= 0x10
      if (pressed.has('ShiftRight')) bits |= 0x20
      if (pressed.has('AltRight')) bits |= 0x40
      if (pressed.has('MetaRight')) bits |= 0x80
      return bits
    }

    function isReleaseShortcut(event) {
      return event.code === 'Escape' && event.shiftKey && !event.altKey && !event.ctrlKey && !event.metaKey
    }

    /**
     * 把画布内的指针位置归一化到 0..=65535。
     * @param canvas - 画布元素。
     * @param event - 指针事件。
     * @returns `{ x, y }`。
     */
    function normalizePoint(canvas, event, clampOutside = false) {
      const rect = canvas.getBoundingClientRect()
      if (!rect.width || !rect.height || !canvas.width || !canvas.height) return null
      const scale = Math.min(rect.width / canvas.width, rect.height / canvas.height)
      const width = canvas.width * scale
      const height = canvas.height * scale
      const x = (event.clientX - rect.left - (rect.width - width) / 2) / width
      const y = (event.clientY - rect.top - (rect.height - height) / 2) / height
      if (!clampOutside && (x < 0 || x > 1 || y < 0 || y > 1)) return null
      const clamp = (value) => Math.max(0, Math.min(65535, Math.round(value * 65535)))
      return {
        x: clamp(x),
        y: clamp(y),
      }
    }

    /**
     * 连接被控端 / 回放片段的控制卡片。
     * @param props - state 与操作。
     * @returns 卡片元素。
     */
    function Controls(props) {
      const { t, state, setState, running, onConnect, onDisconnect } = props
      const toggle = (event) => {
        event.preventDefault()
        if (running) void onDisconnect()
        else void onConnect()
      }
      return h('form', { className: 'rcc-card', onSubmit: toggle },
        h('label', { className: 'rcc-row' },
          h('span', { className: 'rcc-label' }, t('source')),
          h('select', {
            className: 'rcc-select', value: state.mode, disabled: running,
            onChange: (event) => { setState({ ...state, mode: event.target.value }) },
          },
          h('option', { value: 'replay' }, t('modeReplay')),
          h('option', { value: 'connect' }, t('modeConnect')),
          h('option', { value: 'signal' }, t('modeSignal')))),
        state.mode === 'signal'
          ? h('label', { className: 'rcc-row' },
            h('span', { className: 'rcc-label' }, t('deviceId')),
            h('input', {
              className: 'rcc-input', value: state.device, disabled: running, spellCheck: false,
              placeholder: '123 456 789',
              onChange: (event) => { setState({ ...state, device: event.target.value }) },
            }))
          : null,
        state.mode === 'replay'
          ? h('label', { className: 'rcc-row' },
            h('span', { className: 'rcc-label' }, t('clip')),
            h('input', {
              className: 'rcc-input', value: state.clip, disabled: running, spellCheck: false,
              onChange: (event) => { setState({ ...state, clip: event.target.value }) },
            }))
          : h('label', { className: 'rcc-row' },
            h('span', { className: 'rcc-label' }, t('address')),
            h('input', {
              className: 'rcc-input', value: state.address, disabled: running, spellCheck: false,
              placeholder: '127.0.0.1:9901',
              onChange: (event) => { setState({ ...state, address: event.target.value }) },
            })),
        h('label', { className: 'rcc-row' },
          h('span', { className: 'rcc-label' }, t('fps')),
          h('input', {
            className: 'rcc-input', type: 'number', min: 1, max: 120, value: state.fps, disabled: running,
            onChange: (event) => { setState({ ...state, fps: Number.parseInt(event.target.value, 10) || 30 }) },
          })),
        h('div', { className: 'rcc-row' },
          h('button', {
            className: running ? 'rcc-btn' : 'rcc-btn rcc-btn-primary', type: 'submit',
          }, running ? t('disconnect') : (state.mode === 'replay' ? t('modeReplay') : t('connect')))),
        state.mode === 'connect' && !running ? h('div', { className: 'rcc-hint' }, t('needHost')) : null,
        state.mode === 'signal' && !running ? h('div', { className: 'rcc-hint' }, t('relayHint')) : null)
    }

    /**
     * 面板正文。
     * @param props - 由右侧栏注入；本面板不使用 tab 信息。
     * @returns 面板元素。
     */
    function Panel(props) {
      const t = props.t
      const [state, setState] = useState({ mode: 'connect', address: '127.0.0.1:9901', device: '', fps: 30, clip: '' })
      const [resolution, setResolution] = useState('auto')
      const activeTarget = useRef(null)
      const display = useRef({ target: '', nativeSize: '', request: 'auto' })
      const fpsSample = useRef(null)
      const [running, setRunning] = useState(false)
      /** 当前连接目标（设备码），用于列表里高亮那一行。 */
      const [activeDevice, setActiveDevice] = useState('')
      const [error, setError] = useState('')
      const [notice, setNotice] = useState('')
      const [decoderLabel, setDecoderLabel] = useState('')
      const [metrics, setMetrics] = useState({ received: 0, decoded: 0, dropped: 0, errors: 0, size: '' })

      const [devices, setDevices] = useState([])
      const [devicesNote, setDevicesNote] = useState('')
      const [serverUrl, setServerUrl] = useState('')
      /** rc-server 会话：未配置地址或未登录时，主体显示设置卡片而不是设备列表。 */
      const [session, setSession] = useState({ server: '', authorized: false, email: '' })
      /** 设置卡片里的输入与提示。 */
      const [setup, setSetup] = useState({ server: '', email: '', password: '', note: '', busy: false })
      /** 是否显示离线设备（默认只显示在线：中继会话结束时控制端会留下离线行）。 */
      const [showOffline, setShowOffline] = useState(false)
      /** 被控端开关状态（装好插件即可把本机变成可被控的机器）。 */
      const [hostState, setHostState] = useState({ enabled: false, running: false, deviceId: '', backend: '', error: '', source: '', platform: '', log: [] })
      /** 本次会话实际走的链路（direct / p2p / relay），由 Host 半边用 route 事件回传。 */
      const [route, setRoute] = useState('')
      /** 画面铺满：隐藏左栏与高级区；进入时顺带尝试浏览器全屏，Esc 退出。 */
      const [fill, setFill] = useState(false)
      const frameRef = useRef(null)
      /** 左侧抽屉（高级与手动）。 */
      const [drawer, setDrawer] = useState(false)
      const drawerTriggerRef = useRef(null)
      const drawerCloseRef = useRef(null)

      /** 打开抽屉并把焦点移进去。 */
      const openDrawer = useCallback(() => {
        setDrawer(true)
        setTimeout(() => { drawerCloseRef.current?.focus?.() }, 0)
      }, [])

      /** 关闭抽屉，焦点还给触发按钮。 */
      const closeDrawer = useCallback(() => {
        setDrawer(false)
        setTimeout(() => { drawerTriggerRef.current?.focus?.() }, 0)
      }, [])

      // Esc 关抽屉（与铺满互不影响）。
      useEffect(() => {
        if (!drawer) return undefined
        const onKeyDown = (event) => {
          if (event.key === 'Escape') { event.preventDefault(); closeDrawer() }
        }
        document.addEventListener('keydown', onKeyDown)
        return () => { document.removeEventListener('keydown', onKeyDown) }
      }, [drawer, closeDrawer])
      const [sendInputEnabled, setSendInputEnabled] = useState(true)
      const [inputSent, setInputSent] = useState(0)
      const [inputNotice, setInputNotice] = useState('')
      const sendRef = useRef(true)
      const sessionRef = useRef(false)
      const queueRef = useRef([])
      const timerRef = useRef(null)
      const modsRef = useRef(new Set())
      const inputSentRef = useRef(0)
      const canvasRef = useRef(null)
      const decoderRef = useRef(null)
      const abortRef = useRef(null)
      const ownerRef = useRef(null)
      const mountedRef = useRef(true)
      const statsRef = useRef({ received: 0, decoded: 0, dropped: 0, errors: 0, size: '' })
      const frameIndex = useRef(0)
      /** 当前视频流的 H.264 参数集，断开时清空。 */
      const parameterSets = useRef({})
      const configuringRef = useRef(false)
      const supported = typeof window !== 'undefined' && typeof window.VideoDecoder === 'function'

      const publish = useCallback(() => {
        const now = performance.now()
        const decoded = statsRef.current.decoded
        const last = fpsSample.current
        const fps = last && now > last.time && decoded > 0
          ? Math.round((decoded - last.decoded) * 1000 / (now - last.time)) : null
        fpsSample.current = { time: now, decoded }
        setMetrics({ ...statsRef.current, fps })
      }, [])

      useEffect(() => {
        const timer = setInterval(publish, 500)
        return () => { clearInterval(timer) }
      }, [publish])

      /** 拉一次「我的设备」。 */
      const loadDevices = useCallback(async () => {
        try {
          const response = await fetch(DEVICES)
          if (!response.ok) {
            setDevicesNote(response.status === 401 ? t('fenceError') : `HTTP ${String(response.status)}`)
            return
          }
          const payload = await response.json()
          setServerUrl(typeof payload.server === 'string' ? payload.server : '')
          setSession((current) => ({
            ...current,
            server: typeof payload.server === 'string' && payload.server !== '' ? payload.server : current.server,
            authorized: payload.authorized === true,
          }))
          // 在线的排前面：离线行（含自己上次中继时的临时注册）不该抢注意力。
          const list = Array.isArray(payload.devices) ? payload.devices : []
          setDevices([...list].sort((a, b) => Number(b.online === true) - Number(a.online === true)))
          if (payload.authorized !== true) setDevicesNote(t('needSignIn'))
          else if (typeof payload.error === 'string' && payload.error !== '') setDevicesNote(payload.error)
          else setDevicesNote('')
        } catch (cause) {
          setDevicesNote(cause instanceof Error ? cause.message : String(cause))
        }
      }, [t])


      /** 读一次会话状态（地址 + 是否已登录）。 */
      const refreshSession = useCallback(async () => {
        try {
          const response = await fetch(SESSION)
          if (!response.ok) return
          const payload = await response.json()
          const next = {
            server: typeof payload.server === 'string' ? payload.server : '',
            authorized: payload.authorized === true,
            email: typeof payload.email === 'string' ? payload.email : '',
          }
          setSession(next)
          setSetup((current) => ({ ...current, server: current.server === '' ? next.server : current.server }))
        } catch { /* 网络失败就维持现状 */ }
      }, [])

      /** 保存服务器地址（写进插件配置）；换服务器后必须重新登录。 */
      const saveServer = useCallback(async () => {
        const server = setup.server.trim()
        if (server === '') {
          setSetup((current) => ({ ...current, note: t('needServer') }))
          return
        }
        setSetup((current) => ({ ...current, busy: true, note: '' }))
        try {
          const response = await fetch(SERVER, {
            method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ server }),
          })
          const text = await response.text()
          if (!response.ok) {
            setSetup((current) => ({ ...current, busy: false, note: text }))
            return
          }
          const payload = JSON.parse(text)
          const normalized = typeof payload.server === 'string' ? payload.server : server
          setSession({ server: normalized, authorized: false, email: '' })
          setSetup((current) => ({
            ...current, busy: false, server: normalized,
            note: typeof payload.note === 'string' && payload.note !== '' ? payload.note : t('serverSaved'),
          }))
          setDevices([])
          setDevicesNote(t('needSignIn'))
        } catch (cause) {
          setSetup((current) => ({ ...current, busy: false, note: cause instanceof Error ? cause.message : String(cause) }))
        }
      }, [setup.server, t])

      /** 登录：成功后拉一次设备列表。 */
      const signIn = useCallback(async () => {
        const email = setup.email.trim()
        if (setup.server.trim() === '') {
          setSetup((current) => ({ ...current, note: t('needServer') }))
          return
        }
        if (email === '' || setup.password === '') {
          setSetup((current) => ({ ...current, note: t('needCredentials') }))
          return
        }
        setSetup((current) => ({ ...current, busy: true, note: '' }))
        try {
          const response = await fetch(LOGIN, {
            method: 'POST',
            headers: { 'content-type': 'application/json' },
            body: JSON.stringify({ email, password: setup.password }),
          })
          const text = await response.text()
          if (!response.ok) {
            setSetup((current) => ({ ...current, busy: false, note: text }))
            return
          }
          const payload = JSON.parse(text)
          if (payload.mustChangePassword === true) {
            setSetup((current) => ({ ...current, busy: false, password: '', note: t('mustChangePassword') }))
            return
          }
          setSession({ server: setup.server.trim(), authorized: true, email: typeof payload.email === 'string' ? payload.email : email })
          setSetup((current) => ({ ...current, busy: false, password: '', note: '' }))
          await loadDevices()
        } catch (cause) {
          setSetup((current) => ({ ...current, busy: false, note: cause instanceof Error ? cause.message : String(cause) }))
        }
      }, [loadDevices, setup.email, setup.password, setup.server, t])

      /** 读一次被控端状态。 */
      const refreshHost = useCallback(async () => {
        try {
          const response = await fetch(HOST)
          if (!response.ok) return
          setHostState(await response.json())
        } catch { /* 网络失败就维持现状 */ }
      }, [])

      /** 开关被控端。 */
      const toggleHost = useCallback(async (enabled) => {
        setHostState((current) => ({ ...current, enabled }))
        try {
          const response = await fetch(HOST, {
            method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ enabled }),
          })
          if (response.ok) {
            const update = await response.json()
            setHostState((current) => ({ ...current, ...update }))
          }
          else setHostState((current) => ({ ...current, enabled: !enabled, error: `HTTP ${String(response.status)}` }))
        } catch (cause) {
          setHostState((current) => ({ ...current, enabled: !enabled, error: cause instanceof Error ? cause.message : String(cause) }))
        }
      }, [])

      /** 退出登录（清共享凭据记录）。 */
      const signOut = useCallback(async () => {
        try {
          await fetch(LOGOUT, { method: 'POST' })
        } catch { /* 忽略网络错误 */ }
        setSession((current) => ({ ...current, authorized: false, email: '' }))
        setDevices([])
        setDevicesNote(t('needSignIn'))
      }, [t])

      useEffect(() => { void refreshSession() }, [refreshSession])
      useEffect(() => { void refreshHost() }, [refreshHost])

      /** 进入/退出「画面铺满」。 */
      const toggleFill = useCallback(async () => {
        if (fill) {
          setFill(false)
          if (typeof document !== 'undefined' && document.fullscreenElement != null) {
            await document.exitFullscreen().catch(() => {})
          }
          return
        }
        setFill(true)
        const frame = frameRef.current
        if (frame != null && typeof frame.requestFullscreen === 'function' && document.fullscreenElement == null) {
          try { await frame.requestFullscreen() } catch { /* 浏览器不允许就只铺满面板 */ }
        }
      }, [fill])

      // Esc 退出铺满；浏览器自己退出全屏时同步状态。
      useEffect(() => {
        if (!fill) return undefined
        const onKeyDown = (event) => {
          if (event.key === 'Escape') { event.preventDefault(); void toggleFill() }
        }
        const onFullscreenChange = () => {
          if (document.fullscreenElement == null) setFill(false)
        }
        document.addEventListener('keydown', onKeyDown)
        document.addEventListener('fullscreenchange', onFullscreenChange)
        return () => {
          document.removeEventListener('keydown', onKeyDown)
          document.removeEventListener('fullscreenchange', onFullscreenChange)
        }
      }, [fill, toggleFill])

      const teardown = useCallback((controller) => {
        // 只收尾「当前」会话：断开后立刻重连时，旧 connect 的 finally 不能把新会话关掉。
        if (controller !== undefined && abortRef.current !== controller) return
        flushInput()
        releaseModifiers()
        sessionRef.current = false
        ownerRef.current = null
        activeTarget.current = null
        setActiveDevice('')
        setRoute('')
        const current = abortRef.current
        abortRef.current = null
        if (current !== null) current.abort()
        parameterSets.current = {}
        configuringRef.current = false
        const decoder = decoderRef.current
        decoderRef.current = null
        if (decoder !== null) {
          try { decoder.close() } catch { /* 已关闭 */ }
        }
        if (frameRef.current && document.fullscreenElement === frameRef.current) {
          void document.exitFullscreen().catch(() => {})
        }
        if (mountedRef.current) {
          setFill(false)
          setRunning(false)
        }
        // 依赖留空是有意的：teardown 只碰 refs 与 setState，而它调用的
        // flushInput/releaseModifiers 在本函数体内更晚才初始化——把它们的
        // 标识放进依赖数组会在渲染期触发 TDZ ReferenceError，面板直接崩。
      }, [])

      useEffect(() => {
        mountedRef.current = true
        void loadDevices()
        return () => { mountedRef.current = false; teardown() }
      }, [teardown, loadDevices])

      /** 画一帧到 canvas（尺寸变化时才改画布尺寸）。 */
      const draw = useCallback((frame) => {
        const width = frame.displayWidth
        const height = frame.displayHeight
        if (!(width > 0 && height > 0)) return
        statsRef.current.size = `${width}×${height}`
        if (display.current.request === 'auto') display.current.nativeSize = `${width}x${height}`
        const canvas = canvasRef.current
        if (canvas === null) return
        if (canvas.width !== width || canvas.height !== height) {
          canvas.width = width
          canvas.height = height
        }
        const context = canvas.getContext('2d')
        if (context !== null) context.drawImage(frame, 0, 0)
      }, [])

      /** 把一帧交给已配置好的解码器。 */
      const decodeChunk = useCallback((decoder, payload) => {
        const key = hasIdr(payload)
        try {
          decoder.decode(new window.EncodedVideoChunk({
            type: key ? 'key' : 'delta',
            timestamp: Math.round((frameIndex.current * 1_000_000) / Math.max(1, state.fps)),
            data: payload,
          }))
          frameIndex.current += 1
        } catch (cause) {
          statsRef.current.errors += 1
          setError(`decode: ${cause instanceof Error ? cause.message : String(cause)}`)
        }
      }, [state.fps])

      /**
       * 挑一个浏览器确实支持的 codec 串：先试 SPS 里解析出来的，再退回常见取值。
       * @param parsed - 从 SPS 拼出的 `avc1.PPCCLL`。
       * @returns 可用的 codec 串。
       */
      const pickCodec = useCallback(async (parsed) => {
        const candidates = [parsed, 'avc1.42E01E', 'avc1.4D401E', 'avc1.64001E']
        if (typeof window.VideoDecoder.isConfigSupported !== 'function') return parsed
        for (const codec of candidates) {
          try {
            const support = await window.VideoDecoder.isConfigSupported({ codec, avc: { format: 'annexb' } })
            if (support?.supported === true) return codec
          } catch {
            // 试下一个候选
          }
        }
        return parsed
      }, [])

      useEffect(() => { sendRef.current = sendInputEnabled }, [sendInputEnabled])

      /** 把一批事件 POST 给 Host。 */
      const postInput = useCallback(async (events) => {
        const owner = ownerRef.current
        try {
          const response = await fetch(INPUT, {
            method: 'POST',
            headers: { 'content-type': 'application/json' },
            body: JSON.stringify({ events }),
          })
          if (ownerRef.current !== owner) return
          if (response.status === 409) {
            setInputNotice(t('inputNone'))
            return
          }
          if (!response.ok) {
            setInputNotice(`input HTTP ${String(response.status)}`)
            return
          }
          inputSentRef.current += events.length
          setInputSent(inputSentRef.current)
          setInputNotice('')
        } catch (cause) {
          if (ownerRef.current === owner) setInputNotice(cause instanceof Error ? cause.message : String(cause))
        }
      }, [t])

      /** 冲刷队列：一次 POST 带走整批。 */
      const flushInput = useCallback(() => {
        if (timerRef.current !== null) {
          clearTimeout(timerRef.current)
          timerRef.current = null
        }
        const batch = queueRef.current
        queueRef.current = []
        if (batch.length > 0) void postInput(batch)
      }, [postInput])

      /**
       * 入队一批事件；含按键/按钮/滚轮时立即冲刷（这些是关键事件），
       * 纯移动攒到下一拍，避免鼠标一动就一个请求。
       * @param events - 事件数组。
       */
      const pushInput = useCallback((events) => {
        if (!sendRef.current || !sessionRef.current || events.length === 0) return
        queueRef.current.push(...events)
        const critical = events.some((event) => event.t !== 'mm')
        if (critical) flushInput()
        else if (timerRef.current === null) timerRef.current = setTimeout(flushInput, 8)
      }, [flushInput])

      /** 松开所有还按着的修饰键，避免远端出现卡住的 Cmd/Shift。 */
      const releaseModifiers = useCallback(() => {
        const mods = modsRef.current
        if (mods.size === 0) return
        const events = []
        for (const code of mods) {
          const scancode = SCANCODES[code]
          if (scancode !== undefined) events.push({ t: 'k', sc: scancode, p: false, mod: 0 })
        }
        mods.clear()
        if (events.length > 0) pushInput(events)
      }, [pushInput])

      /** 指针移动。 */
      const onPointerMove = useCallback((event) => {
        const canvas = canvasRef.current
        if (canvas === null || !sendRef.current) return
        const point = normalizePoint(canvas, event, canvas.hasPointerCapture(event.pointerId) && event.buttons !== 0)
        if (point === null) return
        pushInput([{ t: 'mm', x: point.x, y: point.y }])
      }, [pushInput])

      /** 指针按下/抬起（含指针捕获，拖出画布也不丢抬起）。 */
      const pointerButton = useCallback((event, pressed) => {
        const canvas = canvasRef.current
        if (canvas === null) return
        const point = normalizePoint(canvas, event, !pressed && canvas.hasPointerCapture(event.pointerId))
        if (point === null) return
        if (pressed) {
          canvas.focus()
          try { canvas.setPointerCapture(event.pointerId) } catch { /* 某些环境不支持 */ }
        }
        if (!sendRef.current) return
        const button = event.button === 2 ? 1 : event.button === 1 ? 2 : 0
        pushInput([{ t: 'mm', x: point.x, y: point.y }, { t: 'mb', b: button, p: pressed }])
      }, [pushInput])

      /** Shift+Escape 将焦点还给面板；其它按键继续转发。 */
      const onKey = useCallback((event, pressed) => {
        if (isReleaseShortcut(event)) {
          event.preventDefault()
          event.stopPropagation()
          if (pressed) {
            releaseModifiers()
            const exited = fill ? toggleFill() : Promise.resolve()
            void exited.then(() => drawerTriggerRef.current?.focus())
          }
          return
        }
        if (!sendRef.current) return
        const scancode = SCANCODES[event.code]
        if (scancode === undefined) {
          if (pressed) statsRef.current.droppedKeys = (statsRef.current.droppedKeys ?? 0) + 1
          return
        }
        if (pressed) modsRef.current.add(event.code)
        else modsRef.current.delete(event.code)
        const modifiers = modifierBits(modsRef.current)
        // 吃掉默认行为：否则空格/方向键会滚页面，快捷键会打到宿主 UI 上。
        event.preventDefault()
        pushInput([{ t: 'k', sc: scancode, p: pressed, mod: modifiers }])
        if (!pressed && MODIFIER_CODES.has(event.code)) modsRef.current.delete(event.code)
      }, [pushInput, releaseModifiers, fill, toggleFill])

      /** 滚轮：换算成「行」，正=向上/向右。 */
      const onWheelRef = useRef(null)
      useEffect(() => {
        onWheelRef.current = (event) => {
          if (!sendRef.current) return
          const step = (value) => Math.max(-3, Math.min(3, -Math.sign(value)))
          const events = []
          if (event.deltaY !== 0) events.push({ t: 'mw', d: step(event.deltaY), h: false })
          if (event.deltaX !== 0) events.push({ t: 'mw', d: step(event.deltaX), h: true })
          if (events.length > 0) {
            event.preventDefault()
            pushInput(events)
          }
        }
      }, [pushInput])

      useEffect(() => {
        const canvas = canvasRef.current
        if (canvas === null) return undefined
        const handler = (event) => { onWheelRef.current?.(event) }
        // 必须手动注册 non-passive：React 的 onWheel 是 passive 的，preventDefault 无效。
        canvas.addEventListener('wheel', handler, { passive: false })
        return () => { canvas.removeEventListener('wheel', handler) }
      }, [])

      /** 处理一帧编码数据：首帧负责建解码器，其余按序送入。 */
      const feed = useCallback((payload) => {
        statsRef.current.received += 1
        const decoder = decoderRef.current
        if (decoder !== null) {
          decodeChunk(decoder, payload)
          return
        }
        const frame = startupFrame(payload, parameterSets.current)
        if (frame === null) {
          statsRef.current.dropped += 1
          return
        }
        const codec = codecFromAnnexB(frame)
        if (codec === undefined) {
          statsRef.current.dropped += 1
          return
        }
        if (configuringRef.current) return
        configuringRef.current = true
        const owner = ownerRef.current
        void (async () => {
          const chosen = await pickCodec(codec)
          // await 期间会话可能已经断开或被替换：那时不能再建解码器。
          if (ownerRef.current !== owner) return
          let created = null
          try {
            created = new window.VideoDecoder({
              output: (frame) => {
                try {
                  if (ownerRef.current !== owner) return
                  draw(frame)
                  if (!statsRef.current.decoded) setNotice('')
                  statsRef.current.decoded += 1
                } finally {
                  frame.close()
                }
              },
              error: (cause) => {
                if (ownerRef.current !== owner) return
                statsRef.current.errors += 1
                setError(`${t('decodeFailed')}: ${cause?.message ?? String(cause)}`)
              },
            })
            created.configure({ codec: chosen, avc: { format: 'annexb' }, optimizeForLatency: true })
          } catch (cause) {
            try { created?.close() } catch { /* 已关闭 */ }
            // 下个关键帧可以重新尝试初始化。
            configuringRef.current = false
            statsRef.current.errors += 1
            setError(`${t('decodeFailed')}: ${cause instanceof Error ? cause.message : String(cause)}`)
            return
          }
          decoderRef.current = created
          setDecoderLabel(chosen)
          configuringRef.current = false
          decodeChunk(created, frame)
        })()
      }, [decodeChunk, draw, pickCodec, t])

      /** 读流并按 `[kind u8][len u32 BE][payload]` 拆帧。 */
      const pump = useCallback(async (response, signal) => {
        const reader = response.body.getReader()
        let buffer = new Uint8Array(0)
        for (;;) {
          if (signal.aborted) { await reader.cancel().catch(() => {}); return }
          const { value, done } = await reader.read()
          if (done || signal.aborted) return
          if (value === undefined) continue
          if (buffer.length + value.length > MAX_BUFFER_BYTES) {
            statsRef.current.errors += 1
            setError(`${t('streamError')}: buffer overflow`)
            buffer = new Uint8Array(0)
            continue
          }
          const merged = new Uint8Array(buffer.length + value.length)
          merged.set(buffer, 0)
          merged.set(value, buffer.length)
          buffer = merged
          for (;;) {
            if (buffer.length < 5) break
            const length = buffer[1] * 16777216 + (buffer[2] << 16) + (buffer[3] << 8) + buffer[4]
            if (length > MAX_FRAME_BYTES) {
              // 长度头不合理：流已经错位，丢弃并如实报错，别把内存吃光。
              statsRef.current.errors += 1
              setError(`${t('streamError')}: frame length ${String(length)} out of range`)
              buffer = new Uint8Array(0)
              break
            }
            if (buffer.length < 5 + length) break
            const kind = buffer[0]
            const payload = buffer.slice(5, 5 + length)
            buffer = buffer.slice(5 + length)
            if (kind === 2) feed(payload)
            else if (kind === 1) {
              try {
                const event = JSON.parse(new TextDecoder().decode(payload))
                if (event?.event === 'route' && typeof event.route === 'string') setRoute(event.route)
              } catch { /* 非 JSON 状态行忽略 */ }
            }
          }
        }
      }, [feed])

      const connect = useCallback(async (target = {}, requestedSize = resolution, message = '') => {
        const mode = target.signalDevice ? 'signal' : target.address ? 'connect' : state.mode
        const address = (target.address ?? state.address).trim()
        const deviceId = mode === 'signal' ? (target.signalDevice ?? state.device).replace(/\s+/g, '') : target.deviceId
        const displayTarget = deviceId || address
        if (display.current.target !== displayTarget) display.current.nativeSize = ''
        display.current.target = displayTarget
        display.current.request = mode === 'replay' ? '' : requestedSize
        const addresses = target.addresses ?? []
        const tried = [...(target.tried ?? []), address]
        if (abortRef.current) teardown()
        const controller = new AbortController()
        abortRef.current = controller
        ownerRef.current = controller
        activeTarget.current = mode === 'signal' ? { signalDevice: deviceId } :
          mode === 'connect' ? { address, deviceId, addresses, tried: target.tried } : null
        sessionRef.current = mode !== 'replay'
        setState((current) => ({ ...current, mode, address, ...(mode === 'signal' ? { device: deviceId } : {}) }))
        setRunning(true)
        setActiveDevice(deviceId ?? '')
        setDecoderLabel('')
        inputSentRef.current = 0
        setInputSent(0)
        setInputNotice('')
        setError('')
        setNotice(message)
        statsRef.current = { received: 0, decoded: 0, dropped: 0, errors: 0, size: '' }
        frameIndex.current = 0
        fpsSample.current = null
        publish()
        const canvas = canvasRef.current
        if (canvas) canvas.getContext('2d')?.clearRect(0, 0, canvas.width, canvas.height)
        const query = new URLSearchParams({ mode, fps: String(state.fps) })
        if (mode === 'replay') {
          if (state.clip.trim()) query.set('file', state.clip.trim())
        } else {
          query.set('resolution', requestedSize)
          query.set('duration', '3600s')
          if (mode === 'signal') query.set('device', deviceId)
          else query.set('connect', address)
        }
        try {
          const response = await fetch(`${STREAM}?${query}`, { signal: controller.signal })
          if (!response.ok || response.body === null) {
            if (response.status === 401) throw new Error(FENCE_401)
            const text = await response.text().catch(() => '')
            let detail = text
            try {
              const parsed = JSON.parse(text)
              detail = parsed.detail || parsed.error || text
            } catch { /* 非 JSON，直接用原文 */ }
            throw new Error(`HTTP ${response.status}${detail === '' ? '' : `：${String(detail).slice(0, 300)}`}`)
          }
          await pump(response, controller.signal)
          if (controller.signal.aborted) return
          if (!statsRef.current.decoded) throw new Error(t('streamError'))
          setNotice(t('sessionEnded'))
        } catch (cause) {
          if (controller.signal.aborted) return
          const reason = cause instanceof Error ? cause.message : String(cause)
          const next = reason === FENCE_401 || reason.includes('device_disabled') ? null : nextAttempt({ mode, deviceId, addresses, tried })
          if (next) {
            void connect({ ...next, deviceId, addresses, tried }, requestedSize, t(next.viaRelay ? 'retryRelay' : 'retryAddress'))
          } else {
            setNotice('')
            setError(reason === FENCE_401 ? t('fenceError') : reason)
          }
        } finally {
          if (abortRef.current === controller) {
            teardown(controller)
            void loadDevices()
          }
        }
      }, [loadDevices, publish, pump, resolution, state, t, teardown])

      const currentSize = running ? metrics.size.replace('×', 'x') : ''
      const sizes = [...new Set([display.current.nativeSize, currentSize, ...RESOLUTIONS].filter(Boolean))]
      function changeResolution(event) {
        const value = event.target.value
        if (!sizes.includes(value)) return
        setResolution(value)
        if (activeTarget.current) void connect(activeTarget.current, value, t('reconnecting'))
      }

      const children = [h(Styles, { key: 'styles' })]
      const stageText = running
        ? (metrics.decoded > 0 ? t('watching') : (metrics.received > 0 ? t('waitingKey') : t('connecting')))
        : t('idle')
      const dotClass = running
        ? (metrics.decoded > 0 ? 'rcc-dot rcc-dot-live' : 'rcc-dot rcc-dot-warn')
        : 'rcc-dot'
      const metric = (label, value) => h('div', { className: 'rcc-metric', key: label },
        h('div', { className: 'rcc-metric-value' }, value),
        h('div', { className: 'rcc-metric-label' }, label))

      // 默认只看在线的：控制端每次走中继都会临时登记一行，会话结束就变离线，
      // 全列出来只会淹没有用信息（想看历史的勾选「显示离线设备」）。
      const visibleDevices = showOffline ? devices : devices.filter((device) => device.online === true)
      const hiddenCount = devices.length - visibleDevices.length
      const deviceRows = visibleDevices.map((device) => {
        const addresses = Array.isArray(device.lanAddrs) ? device.lanAddrs.filter((item) => typeof item === 'string' && item !== '') : []
        const deviceId = String(device.deviceId ?? '')
        const name = typeof device.alias === 'string' && device.alias !== '' ? device.alias : String(device.hostname ?? deviceId)
        const { isSelf, connectable } = deviceRowState(device, hostState.deviceId, running)
        const active = !isSelf && activeDevice === deviceId && deviceId !== ''
        const route = addresses.length > 0 ? t('routeDirectFirst') : t('routeRelayFirst')
        const status = device.allowRemoteControl === false ? t('controlDisabled')
          : device.online !== true ? t('deviceOffline') : running ? t('busy') : route
        const meta = [deviceId, device.os, addresses[0] ?? '', connectable ? status : '']
          .filter((part) => typeof part === 'string' && part !== '').join(' · ')
        // 整行就是一个"连接"动作：有地址先直连、失败自动降级中继；没有地址直接走中继。
        const className = ['rcc-row', isSelf ? 'rcc-row-self' : '', active ? 'rcc-row-active' : '']
          .filter((part) => part !== '').join(' ')
        return h('li', {
          className,
          key: deviceId === '' ? name : deviceId,
          // 本机那条不是按钮：不可点、不可聚焦，读屏也不该把它念成"连接 X"。
          role: isSelf ? 'listitem' : 'button',
          tabIndex: connectable ? 0 : -1,
          'aria-disabled': connectable ? undefined : 'true',
          'aria-label': isSelf ? `${t('thisMachine')} ${name}` : connectable ? `${t('connect')} ${name}` : `${name} · ${status}`,
          title: isSelf
            ? t('selfHint')
            : !connectable ? status : (addresses.length > 0 ? addresses.join(' → ') : t('relayHint')),
          onClick: isSelf ? undefined : () => { if (connectable) void connect(firstAttempt(device)) },
          onKeyDown: isSelf
            ? undefined
            : (event) => {
                if (!connectable) return
                if (event.key === 'Enter' || event.key === ' ') {
                  event.preventDefault()
                  void connect(firstAttempt(device))
                }
              },
        },
        h('span', { className: device.online === true ? 'rcc-dot rcc-dot-live' : 'rcc-dot' }),
        h('div', { className: 'rcc-row-main' },
          h('span', { className: 'rcc-row-title' },
            name,
            isSelf ? h('span', { className: 'rcc-self-chip', key: 'self' }, t('thisMachine')) : null),
          h('span', { className: 'rcc-row-meta', title: meta }, meta),
          !isSelf && !connectable ? h('span', { className: 'rcc-hint' }, status) : null),
        h('span', { className: 'rcc-row-go' }, connectable ? '›' : ''))
      })

      children.push(h('header', { className: 'rcc-head', key: 'head' },
        h('div', { className: 'rcc-head-main' },
          h('span', { className: dotClass }),
          h('div', { className: 'rcc-head-text' },
            h('span', { className: 'rcc-title' }, t('title')),
            // 服务器 · 已登录邮箱 · 链路，压成一行小字：状态一眼可见又不占地方。
            h('span', { className: 'rcc-sub' },
              [
                session.server === '' ? t('notConfigured') : session.server,
                session.authorized ? `${t('signedInAs')} ${session.email}` : '',
                route === '' ? '' : (route === 'direct' ? t('routeDirect') : (route === 'p2p' ? t('routeP2P') : t('routeRelay'))),
              ].filter((part) => part !== '').join(' · ')))),
        h('div', { className: 'rcc-head-actions' },
          (session.authorized || running) && state.mode !== 'replay' ? h('div', { className: 'rcc-resolution' },
            h('span', { className: 'rcc-fps' }, `${running ? (metrics.fps ?? '—') : '—'} FPS`),
            h('select', { className: 'rcc-select', value: currentSize, onChange: changeResolution, 'aria-label': t('resolution'), title: t('reconnectHint') },
              !currentSize ? h('option', { value: '', disabled: true, hidden: true }, '—') : null,
              sizes.map(size => h('option', { key: size, value: size }, size.replace('x', ' × ')))))
            : running && metrics.size ? h('span', { className: 'rcc-chip' }, `${metrics.fps ?? '—'} FPS · ${metrics.size}`) : null,
          h('button', {
            className: 'rcc-icon-btn', type: 'button', title: t('refresh'), 'aria-label': t('refresh'),
            onClick: () => { void loadDevices() },
          }, '⟳'),
          h('button', {
            className: 'rcc-icon-btn', type: 'button', ref: drawerTriggerRef,
            'aria-label': t('advanced'), 'aria-expanded': drawer ? 'true' : 'false',
            title: t('advanced'),
            onClick: () => { openDrawer() },
          }, '⚙'),
          h('button', {
            className: 'rcc-btn', type: 'button', disabled: !running,
            onClick: () => { teardown(); setActiveDevice('') },
          }, t('disconnect')),
          session.authorized
            ? h('button', { className: 'rcc-btn', type: 'button', onClick: () => { void signOut() } }, t('signOut'))
            : null)))

      const setupCard = h('div', { className: 'rcc-body', key: 'setup-body' },
        h('div', { className: 'rcc-stage-pane' },
          h('div', { className: 'rcc-setup' },
            h('div', { className: 'rcc-setup-title' }, t('setupTitle')),
            h('p', { className: 'rcc-hint' }, t('setupHint')),
            h('label', { className: 'rcc-field' },
              h('span', { className: 'rcc-label' }, t('server')),
              h('input', {
                className: 'rcc-input', value: setup.server, spellCheck: false,
                placeholder: 'https://your-server:21118',
                onChange: (event) => { setSetup((current) => ({ ...current, server: event.target.value })) },
              })),
            h('label', { className: 'rcc-field' },
              h('span', { className: 'rcc-label' }, t('email')),
              h('input', {
                className: 'rcc-input', value: setup.email, spellCheck: false, autoComplete: 'username',
                onChange: (event) => { setSetup((current) => ({ ...current, email: event.target.value })) },
              })),
            h('label', { className: 'rcc-field' },
              h('span', { className: 'rcc-label' }, t('password')),
              h('input', {
                className: 'rcc-input', type: 'password', value: setup.password, autoComplete: 'current-password',
                onChange: (event) => { setSetup((current) => ({ ...current, password: event.target.value })) },
              })),
            h('div', { className: 'rcc-row-actions' },
              h('button', {
                className: 'rcc-btn', type: 'button', disabled: setup.busy,
                onClick: () => { void saveServer() },
              }, t('saveServer')),
              h('button', {
                className: 'rcc-btn rcc-btn-primary', type: 'button', disabled: setup.busy,
                onClick: () => { void signIn() },
              }, t('signIn'))),
            setup.note === '' ? null : h('p', { className: 'rcc-message rcc-notice' }, setup.note))))

      children.push(session.authorized ? h('div', { className: 'rcc-body', key: 'body' },
        h('aside', { className: 'rcc-list-pane' },
          h('div', { className: 'rcc-pane-head' },
            h('span', { className: 'rcc-pane-title' }, t('myDevices')),
            h('span', { className: 'rcc-count' }, String(visibleDevices.length)),
            h('span', { className: 'rcc-spacer' })),
          h('p', { className: 'rcc-hint rcc-pane-hint' }, t('connectHint')),
          visibleDevices.length === 0
            ? h('p', { className: 'rcc-empty' }, devicesNote === '' ? t('noDevices') : devicesNote)
            : h('ul', { className: 'rcc-rows' }, deviceRows),
          hiddenCount > 0 ? h('p', { className: 'rcc-empty' }, `${String(hiddenCount)} ${t('offlineHidden')}`) : null,
          devices.length > 0 && devicesNote !== '' ? h('p', { className: 'rcc-empty' }, devicesNote) : null),
        h('section', { className: 'rcc-stage-pane' },
          h('div', { className: 'rcc-stage', key: 'stage' },
            supported && !running
            ? h('div', { className: 'rcc-standby', key: 'standby' },
              h('div', { className: 'rcc-standby-mark' }),
              h('div', { className: 'rcc-standby-title' }, t('pickDevice')),
              h('p', { className: 'rcc-hint' }, t('hint')))
            : null,
          supported
            ? h('div', { className: running ? 'rcc-frame' : 'rcc-frame rcc-frame-idle', ref: frameRef },
              h('canvas', {
                className: 'rcc-canvas',
                ref: canvasRef,
                width: 640,
                height: 360,
                tabIndex: 0,
                'aria-label': t('focusHint'),
                'aria-keyshortcuts': 'Shift+Escape',
                onPointerMove: (event) => { onPointerMove(event) },
                onPointerDown: (event) => { pointerButton(event, true) },
                onPointerUp: (event) => { pointerButton(event, false) },
                onKeyDown: (event) => { onKey(event, true) },
                onKeyUp: (event) => { onKey(event, false) },
                onBlur: () => { releaseModifiers() },
              }),
              running ? h('button', {
                className: 'rcc-icon-btn rcc-fullscreen', type: 'button',
                title: t(fill ? 'exitFullscreen' : 'fullscreen'), 'aria-label': t(fill ? 'exitFullscreen' : 'fullscreen'),
                onClick: () => { void toggleFill() },
              }, h('svg', { viewBox: '0 0 24 24', width: 20, height: 20, fill: 'none', stroke: 'currentColor', strokeWidth: 2, 'aria-hidden': true },
                h('path', { d: fill ? 'M3 9h6V3m6 0v6h6M3 15h6v6m6 0v-6h6' : 'M9 3H3v6m12-6h6v6M3 15v6h6m6 0h6v-6' }))) : null,
              running
                ? h('div', { className: 'rcc-overlay' }, h('span', { className: 'rcc-chip', key: 'state' }, stageText),
                    sendInputEnabled ? h('span', { className: 'rcc-chip' }, t('focusHint')) : null)
                : null)
            : h('p', { className: 'rcc-message rcc-error' }, t('noWebCodecs'))),
          h('div', { className: 'rcc-status', role: 'status' },
            h('span', { className: dotClass }),
            h('span', null, stageText),
            decoderLabel === '' ? null : h('span', null, '· ' + decoderLabel)),
          error === '' ? null : h('p', { className: 'rcc-message rcc-error' }, error),
          notice === '' ? null : h('p', { className: 'rcc-message rcc-notice' }, notice),
          inputNotice === '' ? null : h('p', { className: 'rcc-message rcc-notice' }, inputNotice),
          (metrics.received > 0 && metrics.decoded === 0 && !error.startsWith(t('decodeFailed')))
            ? h('p', { className: 'rcc-message rcc-notice' }, t('undecodable'))
            : null,
          null)) : setupCard)

      // 高级内容做成**左侧滑出抽屉**：不占常驻高度，关闭时仍挂载（状态不丢）。
      children.push(h('div', {
        className: drawer ? 'rcc-drawer-layer rcc-drawer-open' : 'rcc-drawer-layer',
        key: 'drawer',
        'aria-hidden': drawer ? 'false' : 'true',
      },
      h('div', { className: 'rcc-scrim', onClick: () => { closeDrawer() } }),
      h('aside', {
        className: 'rcc-drawer',
        role: 'dialog',
        'aria-modal': 'true',
        'aria-label': t('advanced'),
      },
      h('header', { className: 'rcc-drawer-head' },
        h('span', { className: 'rcc-drawer-title' }, t('advanced')),
        h('button', {
          className: 'rcc-icon-btn', type: 'button', ref: drawerCloseRef,
          'aria-label': t('closeDrawer'), title: t('closeDrawer'),
          onClick: () => { closeDrawer() },
        }, '×')),
      h('div', { className: 'rcc-drawer-body' },
          h('div', { className: 'rcc-metrics', key: 'metrics' },
            metric(t('frames'), String(metrics.received)),
            metric(t('decoded'), String(metrics.decoded)),
            metric(t('dropped'), String(metrics.dropped)),
            metric(t('decodeErrors'), String(metrics.errors)),
            metric(t('decodedSize'), metrics.size === '' ? '—' : metrics.size),
            metric(t('inputSent'), String(inputSent))),
          h('div', { className: 'rcc-host', key: 'host' },
            hostState.requiresWindowsGui && !hostState.enabled ? null : h('label', { className: 'rcc-check' },
              h('input', {
                type: 'checkbox', checked: hostState.enabled === true,
                disabled: !session.authorized,
                onChange: (event) => { void toggleHost(event.target.checked) },
              }),
              h('span', null, t('hostEnable'))),
            h('div', { className: 'rcc-status' },
              h('span', { className: hostState.running === true ? 'rcc-dot rcc-dot-live' : 'rcc-dot' }),
              h('span', null, hostState.running === true ? t('hostRunning') : t('hostStopped')),
              hostState.deviceId === '' ? null : h('span', null, `· ${t('hostDevice')} ${hostState.deviceId}`),
              hostState.backend === '' ? null : h('span', null, `· ${hostState.backend}`)),
            hostState.enabled !== true || session.authorized ? null : h('p', { className: 'rcc-hint' }, t('hostNeedLogin')),
              h('label', { className: 'rcc-check' },
              h('input', {
                type: 'checkbox', checked: showOffline,
                onChange: (event) => { setShowOffline(event.target.checked) },
              }),
              h('span', null, t('showOffline'))),
          h('p', { className: 'rcc-hint' }, t(hostState.requiresWindowsGui ? 'windowsHostHint' : 'hostHint')),
            (() => {
              const log = Array.isArray(hostState.log) ? hostState.log.join('\n') : ''
              if (/输入注入=真实/.test(log)) return h('p', { className: 'rcc-hint' }, t('hostInjectReal'))
              if (/输入注入=记录型/.test(log)) return h('p', { className: 'rcc-message rcc-error' }, t('hostInjectBlocked'))
              return null
            })(),
            hostState.error === '' ? null : h('p', { className: 'rcc-message rcc-error' }, hostState.error)),
          h(Controls, { key: 'controls', t, state, setState, running, onConnect: connect, onDisconnect: async () => { teardown(); setActiveDevice('') } }),
          h('label', { className: 'rcc-check', key: 'input-toggle' },
            h('input', {
              type: 'checkbox', checked: sendInputEnabled,
              onChange: (event) => { setSendInputEnabled(event.target.checked) },
            }),
            h('span', null, t('inputOn'))),
          h('p', { className: 'rcc-hint' }, t('focusHint'))))))

      return h('div', { className: fill ? 'rcc-root rcc-fill' : 'rcc-root' }, children)
    }

    /**
     * 把面板绑定到当前语言。
     * @param t - 翻译函数。
     * @returns 组件。
     */
    function makePanel(t) {
      return function BoundPanel(props) {
        return h(Panel, { ...props, t })
      }
    }

    return {
      inject: ['slots', 'layout', 'locale', 'sidebarRight', 'sidebarRightTabs', 'shortcuts'],
      // 只给无头测试用：把纯映射函数暴露出来，好在没有浏览器时也能验证扫描码表。
      __test: { startupFrame, normalizePoint, isReleaseShortcut, SCANCODES, modifierBits, MAX_FRAME_BYTES, MAX_BUFFER_BYTES, INPUT, nextAttempt, firstAttempt, deviceRowState, css: CSS },
      apply(ctx) {
        guard('locale', () => {
          ctx.effect(() => ctx.locale.register(NS, { zh: ZH, en: EN }), 'rabbitcontrol-client: copy')
        })
        const t = ctx.locale.bind(NS)
        guard('tab-type', () => {
          ctx.effect(() => ctx.sidebarRightTabs.register({
            id: ID,
            kind: KIND,
            title: () => t('title'),
            guide: [{
              id: 'open',
              order: 70,
              title: () => t('title'),
              description: () => t('description'),
              icon: PanelIcon,
            }],
          }), 'rabbitcontrol-client: tab type')
        })
        guard('shortcut', () => {
          ctx.effect(() => ctx.shortcuts.register({
            id: 'rabbitcontrol.client.open',
            label: () => t('title'),
            aliases: ['remote screen'],
            defaults: {
              'desktop:macos': { code: 'KeyJ', modifiers: ['primary'] },
              'desktop:windows': { code: 'KeyJ', modifiers: ['primary'] },
              'desktop:linux': { code: 'KeyJ', modifiers: ['primary'] },
              'web:macos': { code: 'KeyJ', modifiers: ['primary', 'alt'] },
              'web:windows': { code: 'KeyJ', modifiers: ['primary', 'alt'] },
            },
            regions: ['page', 'editable', 'terminal'],
            modals: [],
            resolve: () => ({
              status: 'handled',
              run: () => {
                // 中央面板是全局的，没有"必须已有会话"这一说；万一面板注册失败就退回右侧标签页。
                try {
                  if (panelReady) ctx.layout.selectPanel(PANEL_ID)
                  else ctx.sidebarRight.openTab(KIND)
                } catch (error) {
                  reportFailure('shortcut-run', error)
                }
              },
            }),
          }), 'rabbitcontrol-client: shortcut')
        })
        guard('panel', () => {
          ctx.effect(() => ctx.slots.inject('main', () => ctx.slots.register({
            name: 'main',
            key: PANEL_ID,
            locale: NS,
          }, makePanel(t))), 'rabbitcontrol-client: main panel')
          panelReady = true
        })
        guard('panel-entry', () => {
          ctx.effect(() => ctx.slots.inject('sidebar.panellist', () => ctx.slots.register({
            name: 'sidebar.panellist',
            id: PANEL_ID,
            order: 20,
            locale: NS,
            label: () => t('panel'),
          }, PanelIcon)), 'rabbitcontrol-client: sidebar entry')
        })
        guard('panel-body', () => {
          ctx.effect(() => ctx.slots.inject('sidebar.right.pane.tab', () => ctx.slots.register(
            { name: 'sidebar.right.pane.tab', key: ID, locale: NS, inject: () => ({}) },
            makePanel(t),
          )), 'rabbitcontrol-client: panel body')
        })
      },
    }
  },
})


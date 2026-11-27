/**
 * RabbitControl 客户端 —— Host 半边。
 *
 * 提供一个流式路由 `GET /rabbitcontrol-client/stream`：拉起 rc-bridge 边车
 * （离线回放或连被控端），把它的成帧输出原样转发给浏览器。帧流协议与边车一致：
 * `[kind u8][len u32 BE][payload]`，kind=1 状态 JSON、kind=2 Annex-B 视频帧。
 *
 * 只用 node: 内置模块。不做解码、不碰 GPU——解码在浏览器侧用 WebCodecs。
 */
import z from '@deepseek-ai/schemastery'
import { appendFileSync, existsSync, writeFileSync } from 'node:fs'
import { isIPv6 } from 'node:net'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { execFile, spawn } from 'node:child_process'
import { createHash, randomBytes } from 'node:crypto'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { promisify } from 'node:util'
const runBridge = promisify(execFile)
import { request as httpRequest } from 'node:http'
import { request as httpsRequest } from 'node:https'

export const inject = ['webServer']


/** 流式路由：与 Web GUI 同源，浏览器 fetch 后读 ReadableStream。 */
const STREAM_PATH = '/rabbitcontrol-client/stream'
/** 设备列表路由：从 rc-server 读「我的设备」，供面板列出并直连。 */
const DEVICES_PATH = '/rabbitcontrol-client/devices'
/** 会话状态：面板据此决定显示设置卡片还是设备列表。 */
const SESSION_PATH = '/rabbitcontrol-client/session'
/** 配置服务器地址（写进插件配置）。 */
const SERVER_PATH = '/rabbitcontrol-client/server'
/** 用 rc-server 账号登录（写共享凭据记录）。 */
const LOGIN_PATH = '/rabbitcontrol-client/login'
/** 退出登录（清共享凭据记录）。 */
const LOGOUT_PATH = '/rabbitcontrol-client/logout'
/** 客户端报错通道：把浏览器侧的注册/渲染错误落到磁盘，便于没有浏览器控制时定位。 */
const REPORT_PATH = '/rabbitcontrol-client/report'
/** 跨平台临时目录：Windows 上没有 /tmp，必须用系统给的那个。 */
const TMP = tmpdir()
/** 找不到边车时的提示：说明包里带哪些平台、该怎么办。 */
const BRIDGE_MISSING = '找不到 rc-bridge 边车：本包自带 macOS(arm64) 与 Windows(x64) 两个版本，'
  + '当前平台没有对应的那个。请在插件配置里把 bridgePath 指到你自己构建的边车（见 README）。当前平台：'
/**
 * 信令路径要用的 CA 落盘位置：**按服务端地址分文件**——换服务端（例如本地 → VPS）时
 * 旧 CA 会把新服务端的 TLS 校验搞挂，这种缓存必须跟着地址走。
 * @param server - 服务端 REST 地址。
 * @returns CA PEM 的缓存路径。
 */
function caCachePath(server) {
  const slug = server.replace(/[^A-Za-z0-9]+/g, '_').replace(/^_+|_+$/g, '')
  return join(TMP, `dsh-rabbitcontrol-ca-${slug}.pem`)
}
/** 设备密钥/证书目录（边车签发 7 天设备证书用）。 */
const SIGNAL_CERT_DIR = join(TMP, 'dsh-rabbitcontrol-certs')
/**
 * 本端信令设备 key 的默认值。同一 profile 固定即可，但**一旦该设备被管理端停用**
 * （`DELETE /api/v1/devices/{id}` 是停用而非删除），这个 key 就再也登记不上
 * （服务端回 `device_disabled`），需要管理员处理。
 */
const SIGNAL_DEVICE_KEY = 'dsh-rabbitcontrol-client'
/** 报错落盘位置。 */
const REPORT_FILE = join(TMP, 'dsh-rabbitcontrol-client.log')
/** 输入路由：面板把键鼠事件 POST 到这里，转成边车的 stdin 成帧消息。 */
const INPUT_PATH = '/rabbitcontrol-client/input'
/** 宿主 → 边车的输入消息类型号，与 rc-bridge/src/input.rs 的 `INPUT_KIND` 一致。 */
const INPUT_KIND = 10
const CLOSE_SESSION_FRAME = Buffer.from([3, 0, 0, 0, 0])
/** 输入请求体上限。 */
const MAX_INPUT_BODY = 64 * 1024
const BUNDLE_DIR = dirname(fileURLToPath(import.meta.url))

function defaultBridge() {
  let name
  if (process.platform === 'darwin' && process.arch === 'arm64') name = 'rc-bridge'
  else if (process.platform === 'win32' && ['x64', 'arm64'].includes(process.arch)) name = 'rc-bridge-win-x64.exe'
  else return ''
  const path = join(BUNDLE_DIR, 'bin', name)
  return existsSync(path) ? path : ''
}
/** 离线回放默认片段（可在插件行 config 覆盖）。 */
const DEFAULT_CLIP = ''   // 回放用的片段由使用者在自己机器上指定（发布包不带绝对路径）
/** rc-server 的 REST 地址（与管理面板同一个部署）。 */
/** 默认**不预设**任何服务器：地址由使用者第一次在面板里填，见「设置卡片」。 */
const DEFAULT_SERVER = ''
/** 与管理面板共用同一条凭据记录：登录一次，两边都能用。 */
const CREDENTIAL_KEY = 'dsh-rabbitcontrol/session'
/** 设备列表最多返回多少台。 */
const MAX_DEVICES = 100
/** 允许的边车参数上限，避免把路由变成任意命令执行。 */
const MAX_ADDRESS_LEN = 64
const ALLOWED_MODES = new Set(['replay', 'connect'])
/** 信令路径的设备码形状。 */
const SIGNAL_DEVICE_ID = /^\d{9}$/

/** 被控端的信令设备 key（与客户端的分开）。 */
const HOST_DEVICE_KEY = 'dsh-rabbitcontrol-host'
/** 被控端路由。 */
const HOST_PATH = '/rabbitcontrol-client/host'

/**
 * 插件行配置：导出 schema 后，插件行里的字段会被校验，并能在设置里按字段编辑。
 * 默认值与下面的常量保持一致。
 */
export const Config = z.object({
  bridgePath: z.string().default('').description('rc-bridge 可执行文件路径；留空则用插件自带的（bin/ 下按平台选）'),
  clipPath: z.string().default(DEFAULT_CLIP).description('「回放测试片段」用的 Annex-B H.264 文件'),
  serverUrl: z.string().default('').description('rc-server 的 REST 地址；留空时在面板里首次配置（推荐）'),
  signalDeviceKey: z.string().default(SIGNAL_DEVICE_KEY).description('本机在 rc-server 上的信令设备 key（停用后请联系管理员）'),
  signalTransport: z.union(['auto', 'p2p', 'relay']).default('auto').description('信令路径的传输偏好：auto = 先打洞（RCP2）失败回退中继（RCR1）；p2p = 只用打洞；relay = 只用中继'),
  insecureTls: z.boolean().default(true).description('接受自签证书；仅对本机/内网自签部署打开'),
  hostMode: z.boolean().default(false).description('作为被控端：本机登记到 rc-server 并把屏幕提供给别人看（需要平台采集/编码后端，当前仅 macOS 支持真实屏幕）'),
  hostSource: z.union(['real', 'clip']).default('real').description('被控端内容：real = 真实屏幕（macOS）；clip = 回放片段'),
  hostDeviceKey: z.string().default(HOST_DEVICE_KEY).description('被控端在本机 rc-server 上的设备 key（与客户端的 signalDeviceKey 分开，避免互相顶掉）'),
})

/** 当前生效的设置。 */
let settings = {
  bridge: defaultBridge(),
  clip: DEFAULT_CLIP,
  server: DEFAULT_SERVER,
  insecureTls: true,
  hostMode: false,
  hostSource: 'real',
  hostDeviceKey: HOST_DEVICE_KEY,
}
/** 被控端子进程与它最近的状态（面板据此显示设备码与后端）。 */
let host = { child: null, deviceId: '', backend: '', error: '', restarts: 0, log: [] }
/** 当前活着的会话（同一时刻只允许一条流）：输入路由要把事件写进它的 stdin。 */
let active = null

/**
 * 把一批事件写进当前会话的 stdin。
 * @param events - `{ events: [...] }` 形式的载荷。
 * @returns 写成功返回 true；没有会话或通道已关返回 false。
 */
function writeInput(events) {
  if (active === null) return 'no_session'
  // 回放模式没有会话，输入无处可去：如实拒绝，而不是让事件悄悄消失。
  if (active.acceptsInput !== true) return 'input_disabled'
  const stdin = active.child.stdin
  if (stdin === null || stdin.destroyed || stdin.writableEnded) return 'no_session'
  const payload = Buffer.from(JSON.stringify(events), 'utf8')
  if (payload.length > MAX_INPUT_BODY) return 'too_large'
  const header = Buffer.alloc(5)
  header[0] = INPUT_KIND
  header.writeUInt32BE(payload.length, 1)
  stdin.write(Buffer.concat([header, payload]))
  return undefined
}

/**
 * 读请求体，带体积上限。
 * @param req - Node 请求。
 * @returns 请求体文本。
 */
function readBody(req, limit = MAX_INPUT_BODY) {
  return new Promise((resolve, reject) => {
    const chunks = []
    let size = 0
    req.on('data', (chunk) => {
      size += chunk.length
      if (size > limit) {
        reject(new Error(`请求体超过 ${limit} 字节`))
        req.destroy()
        return
      }
      chunks.push(chunk)
    })
    req.on('end', () => { resolve(Buffer.concat(chunks).toString('utf8')) })
    req.on('error', reject)
  })
}

/**
 * 校验被控端地址形状。
 * @param value - 查询参数里的地址。
 * @returns 合法的 `host:port`，否则 undefined。
 */
function normalizeAddress(value) {
  if (typeof value !== 'string') return undefined
  const trimmed = value.trim()
  if (trimmed.length === 0 || trimmed.length > MAX_ADDRESS_LEN) return undefined
  const parts = /^(?:[A-Za-z0-9._-]+|\[([a-fA-F0-9:.]+)(?:%(\d{1,10}))?\]):(\d{1,5})$/.exec(trimmed)
  if (!parts || Number(parts[3]) < 1 || Number(parts[3]) > 65535) return undefined
  if (parts[1] && (!isIPv6(parts[1]) || Number(parts[2]) > 0xffffffff)) return undefined
  return trimmed
}

/**
 * 校验文件路径：必须是绝对路径、无换行、且以 .h264 结尾。
 * @param value - 查询参数里的路径。
 * @returns 合法路径，否则 undefined。
 */
function normalizeClip(value) {
  if (typeof value !== 'string') return undefined
  const trimmed = value.trim()
  if (!trimmed.startsWith('/') || trimmed.includes('\n') || trimmed.length > 512) return undefined
  return trimmed.endsWith('.h264') ? trimmed : undefined
}

/**
 * 按查询参数拼出边车命令行。
 * @param query - URLSearchParams。
 * @returns `{ args }` 或 `{ error }`。
 */
function bridgeArgs(query) {
  const mode = query.get('mode') ?? 'replay'
  const resolution = query.get('resolution') ?? 'auto'
  if (mode !== 'replay' && resolution !== 'auto') {
    const match = /^([1-9]\d{0,4})x([1-9]\d{0,4})$/.exec(resolution)
    if (!match || match.slice(1).some(n => Number(n) > 16384 || Number(n) % 2 !== 0)) {
      return { error: '分辨率应为 auto 或偶数宽高，例如 1920x1080' }
    }
  }
  if (mode === 'signal') return { signal: true, resolution }
  if (!ALLOWED_MODES.has(mode)) return { error: `不支持的 mode：${mode}` }
  const fpsRaw = Number.parseInt(query.get('fps') ?? '30', 10)
  const fps = Number.isFinite(fpsRaw) && fpsRaw >= 1 && fpsRaw <= 120 ? fpsRaw : 30
  if (mode === 'replay') {
    const clip = query.get('file') === null ? settings.clip : normalizeClip(query.get('file'))
    if (clip === undefined) return { error: '回放文件必须是绝对路径且以 .h264 结尾' }
    return { args: ['--replay', clip, '--fps', String(fps), '--out', '-'], acceptsInput: false, mode: 'replay' }
  }
  const connect = normalizeAddress(query.get('connect'))
  if (connect === undefined) return { error: '被控端地址须为 host:port 或 [IPv6]:port，端口范围 1–65535' }
  const duration = query.get('duration') ?? '600s'
  if (!/^\d{1,5}(s|ms)?$/.test(duration)) return { error: '会话时长不合法' }
  return { args: ['--connect', connect, '--resolution', resolution, '--fps', String(fps), '--duration', duration, '--out', '-'], acceptsInput: true, mode: 'connect' }
}

/**
 * 准备一次信令（跨网段）会话：取 token、把 CA 落盘，拼出边车命令行。
 *
 * 令牌走环境变量而不是命令行：`ps` 里不该出现 access token。
 * @param ctx - Host 上下文。
 * @param query - 查询参数（`device` 为 9 位设备码）。
 * @returns `{ args, env, acceptsInput }` 或 `{ error }`。
 */
async function prepareSignalSession(ctx, query, resolution) {
  const device = (query.get('device') ?? '').replace(/\s+/g, '')
  if (!SIGNAL_DEVICE_ID.test(device)) return { error: '设备码必须是 9 位数字' }
  if (settings.server === '') return { error: '还没有配置 rc-server 地址：请在面板的「服务器」里填写地址并登录' }
  const token = await accessToken(ctx)
  if (token === undefined) return { error: '需要先登录 rc-server：请在面板的「服务器」里用账号登录' }
  const caPath = caCachePath(settings.server)
  if (!existsSync(caPath)) {
    const ca = await rcRequest('/api/v1/pki/ca')
    const pem = typeof ca.json?.ca_cert_pem === 'string' ? ca.json.ca_cert_pem : ''
    if (pem === '') return { error: `取服务端 CA 失败：${describeFailure(ca)}` }
    try {
      writeFileSync(caPath, pem, { mode: 0o600 })
    } catch (error) {
      return { error: `CA 落盘失败：${error instanceof Error ? error.message : String(error)}` }
    }
  }
  const fpsRaw = Number.parseInt(query.get('fps') ?? '30', 10)
  const fps = Number.isFinite(fpsRaw) && fpsRaw >= 1 && fpsRaw <= 120 ? fpsRaw : 30
  const duration = query.get('duration') ?? '3600s'
  if (!/^\d{1,5}(s|ms)?$/.test(duration)) return { error: '会话时长不合法' }
  return {
    args: ['--signal-target', device, '--signal-server', settings.server,
      '--signal-device-key', settings.signalDeviceKey, '--signal-ca', caPath,
      '--signal-cert-dir', SIGNAL_CERT_DIR, '--transport', settings.signalTransport, '--signal-connect',
      '--resolution', resolution, '--fps', String(fps), '--duration', duration, '--out', '-'],
    env: { ...process.env, RC_SIGNAL_TOKEN: token },
    acceptsInput: true,
  }
}

// stdout pipe chunks can split a 5-byte header or an H.264 access unit at any byte.
// Track only framing metadata, so a late stderr route announcement can be inserted
// between records without buffering or delaying the first video frame.
function frameBoundaries() {
  const header = Buffer.alloc(5)
  let headerBytes = 0
  let payloadLeft = 0
  return chunk => {
    let offset = 0
    let first = headerBytes === 0 && payloadLeft === 0 ? 0 : -1
    while (offset < chunk.length) {
      if (payloadLeft === 0) {
        const bytes = Math.min(5 - headerBytes, chunk.length - offset)
        chunk.copy(header, headerBytes, offset, offset + bytes)
        offset += bytes
        headerBytes += bytes
        if (headerBytes < 5) break
        payloadLeft = header.readUInt32BE(1)
        headerBytes = 0
      }
      const bytes = Math.min(payloadLeft, chunk.length - offset)
      payloadLeft -= bytes
      offset += bytes
      if (payloadLeft === 0 && first < 0) first = offset
    }
    return first
  }
}

/**
 * 流式路由处理器：过框架围栏 → 校验参数 → 拉起边车 → 把 stdout 原样转发。
 * @param ctx - Host 上下文。
 * @param req - Node 请求。
 * @param res - Node 响应。
 */
async function routeHandler(ctx, req, res) {
  let connection
  try {
    connection = ctx.get('connection')
  } catch {
    connection = undefined
  }
  if (connection === undefined) {
    res.writeHead(503, { 'content-type': 'text/plain' })
    res.end('no connection service')
    return
  }
  const rejection = connection.requestRejection(req)
  if (rejection !== undefined) {
    res.writeHead(rejection)
    res.end(rejection === 401 ? 'unauthorized' : 'forbidden')
    return
  }
  if (req.method !== 'GET') {
    res.writeHead(405, { allow: 'GET' })
    res.end('method not allowed')
    return
  }

  const query = new URL(req.url ?? '/', 'http://localhost').searchParams
  const parsed = bridgeArgs(query)
  if (parsed.error !== undefined) {
    res.writeHead(400, { 'content-type': 'text/plain; charset=utf-8' })
    res.end(parsed.error)
    return
  }
  if (settings.bridge === '') {
    res.writeHead(409, { 'content-type': 'text/plain; charset=utf-8' })
    res.end(`${BRIDGE_MISSING}${process.platform}-${process.arch}）`)
    return
  }
  const plan = parsed.signal === true ? await prepareSignalSession(ctx, query, parsed.resolution) : parsed
  // 切换分辨率会中断旧请求；等待信令凭据期间对端可能已经断开。
  if (res.destroyed) return
  if (plan.error !== undefined) {
    res.writeHead(409, { 'content-type': 'text/plain; charset=utf-8' })
    res.end(plan.error)
    return
  }

  // 设备身份固定，旧信令连接退场以后才能用同一身份注册下一次会话。
  while (active !== null) {
    const previous = active
    previous.stop()
    await previous.closed
    if (res.destroyed) return
  }
  const child = spawn(settings.bridge, plan.args, {
    stdio: ['pipe', 'pipe', 'pipe'],
    ...(plan.env === undefined ? {} : { env: plan.env }),
  })
  let stopping = false
  let killTimer
  let killDeadline
  // 首包到达前不写状态行：这样「边车起不来 / 对端没监听」能回一个可读的错误，
  // 而不是先给浏览器 200 再把真相丢进日志。
  let headersSent = false
  let stderrTail = ''
  const sendHead = (status, headers) => {
    if (headersSent) return
    headersSent = true
    res.writeHead(status, headers)
  }
  // 继续消费 stdout，避免浏览器取消读取后，边车卡在输出而收不到结束指令。
  const stop = () => {
    if (stopping) return
    stopping = true
    child.stdout.resume()
    if (!res.destroyed && !res.writableEnded) res.end()
    if (plan.acceptsInput && child.stdin.writable) {
      child.stdin.end(CLOSE_SESSION_FRAME)
      killTimer = setTimeout(() => child.kill('SIGTERM'), 2500)
    } else {
      child.kill('SIGTERM')
    }
    killDeadline = setTimeout(() => child.kill('SIGKILL'), 4000)
  }
  // stdin 可能随边车提前退出而关闭。最终退出及诊断统一从 close 收取。
  child.stdin.on('error', () => {})
  child.once('error', error => { stderrTail = error.message })
  const closed = new Promise(resolve => child.once('close', (code, signal) => {
    clearTimeout(killTimer)
    clearTimeout(killDeadline)
    if (active?.child === child) active = null
    const reason = `bridge exit code=${code} signal=${signal}`
    console.log(`[rabbitcontrol-client] 流结束：${reason}`)
    if (!res.destroyed && !res.writableEnded) {
      if (!headersSent) {
        sendHead(502, { 'content-type': 'application/json; charset=utf-8' })
        res.end(JSON.stringify({ error: 'bridge_failed', detail: stderrTail.trim().slice(-600) || reason }))
      } else res.end()
    }
    resolve()
  }))
  active = { child, stop, closed, acceptsInput: plan.acceptsInput === true }

  // 实际链路（direct / p2p / relay）用 kind=1 的 JSON 行回传，面板据此显示徽标。
  let routeSent = ''
  const emitRoute = (route) => {
    if (route === routeSent || route === '') return
    routeSent = route
    const body = Buffer.from(JSON.stringify({ event: 'route', route }) + '\n', 'utf8')
    const head = Buffer.alloc(5)
    head[0] = 1
    head.writeUInt32BE(body.length, 1)
    res.write(Buffer.concat([head, body]))
  }
  let pendingRoute = plan.mode === 'connect' ? 'direct' : ''
  const boundaryOf = parsed.signal ? frameBoundaries() : () => 0

  child.stdout.on('data', (chunk) => {
    if (stopping || res.destroyed) return
    sendHead(200, {
      'content-type': 'application/octet-stream',
      'cache-control': 'no-store',
      'x-accel-buffering': 'no',
    })
    const boundary = boundaryOf(chunk)
    if (boundary >= 0 && pendingRoute !== routeSent) {
      if (boundary > 0) res.write(chunk.subarray(0, boundary))
      emitRoute(pendingRoute)
      if (boundary < chunk.length) res.write(chunk.subarray(boundary))
    } else res.write(chunk)
    // 手动转发以便观察背压：写不动就暂停边车输出，drain 后继续。
    if (res.writableNeedDrain && child.stdout.isPaused() === false) {
      child.stdout.pause()
      res.once('drain', () => { child.stdout.resume() })
    }
  })
  child.stdout.on('error', stop)
  child.stderr.setEncoding('utf8')
  let partialLine = ''
  let explicitRoutes = false
  child.stderr.on('data', text => {
    stderrTail = (stderrTail + text).slice(-2000)
    const lines = (partialLine + text).split('\n')
    partialLine = lines.pop().slice(-2000)
    for (const line of lines) {
      if (!line.trim()) continue
      console.log(`[rabbitcontrol-client] bridge: ${line.trim()}`)
      if (!parsed.signal) continue
      const committed = /\brc-bridge route=(p2p|relay)\b/.exec(line)
      if (committed) {
        explicitRoutes = true
        pendingRoute = committed[1]
      } else if (!explicitRoutes) {
        // 兼容旧边车。新边车先发明确的初始路径，后台候选不改变徽标。
        if (line.includes('打洞成功')) pendingRoute = 'p2p'
        else if (line.includes('→ 中继')) pendingRoute = 'relay'
      }
    }
  })
  res.once('close', () => { if (active?.child === child) stop() })
}

/**
 * 读一次 rc-server 的 JSON 接口。
 *
 * 用 node:https 而不是 fetch：自签部署（`insecureTls`）需要 `rejectUnauthorized`，
 * 而 fetch/undici 没有这个开关，会直接以 UNABLE_TO_VERIFY_LEAF_SIGNATURE 失败。
 * @param path - `/api/v1/...`。
 * @param options - method / token / body。
 * @returns `{ status, json, text }`；传输失败带 `transportError`。
 */
function rcRequest(path, options = {}) {
  const { method = 'GET', token, body, timeoutMs = 8000 } = options
  return new Promise((resolve) => {
    let url
    try {
      url = new URL(path, `${options.server ?? settings.server}/`)
    } catch {
      resolve({ status: 0, json: undefined, text: '', transportError: `rc-server 地址无效：${settings.server}` })
      return
    }
    const payload = body === undefined ? undefined : Buffer.from(JSON.stringify(body), 'utf8')
    const secure = url.protocol === 'https:'
    const transport = secure ? httpsRequest : httpRequest
    const headers = { accept: 'application/json' }
    if (payload !== undefined) {
      headers['content-type'] = 'application/json'
      headers['content-length'] = String(payload.length)
    }
    if (typeof token === 'string' && token !== '') headers.authorization = `Bearer ${token}`
    const req = transport({
      protocol: url.protocol,
      hostname: url.hostname.replace(/^\[|\]$/g, ''),
      port: url.port === '' ? undefined : url.port,
      path: `${url.pathname}${url.search}`,
      method,
      headers,
      timeout: timeoutMs,
      ...(secure ? { rejectUnauthorized: !settings.insecureTls } : {}),
    }, (res) => {
      const chunks = []
      res.on('data', (chunk) => { chunks.push(chunk) })
      res.on('end', () => {
        const text = Buffer.concat(chunks).toString('utf8')
        let json
        try { json = text === '' ? undefined : JSON.parse(text) } catch { json = undefined }
        resolve({ status: res.statusCode ?? 0, json, text })
      })
    })
    req.on('timeout', () => { req.destroy(new Error(`请求超时（${timeoutMs}ms）`)) })
    req.on('error', (error) => {
      resolve({ status: 0, json: undefined, text: '', transportError: error instanceof Error ? error.message : String(error) })
    })
    if (payload !== undefined) req.write(payload)
    req.end()
  })
}

/**
 * 启动被控端：本机登记到 rc-server，把屏幕（或片段）提供给别人。
 *
 * 复用同一个边车：`--source real` 在 macOS 上是 ScreenCaptureKit + VideoToolbox（插件自带的
 * 边车已带 `real-capture` 特性）；其它平台会以可读错误退出，面板据此提示。
 * @param ctx - Host 上下文。
 */
async function startHost(ctx) {
  if (host.child !== null) return
  if (settings.server === '') {
    host = { ...host, error: '还没有配置 rc-server 地址（先在面板里填地址并登录）' }
    return
  }
  if (settings.bridge === '') {
    host = { ...host, error: '找不到边车可执行文件' }
    return
  }
  const token = await accessToken(ctx)
  if (token === undefined) {
    host = { ...host, error: '需要先在面板里登录 rc-server' }
    return
  }
  // CA 落盘（边车用文件路径校验服务端 TLS），与客户端路径共用同一份缓存。
  const caPath = caCachePath(settings.server)
  if (!existsSync(caPath)) {
    const ca = await rcRequest('/api/v1/pki/ca')
    const pem = typeof ca.json?.ca_cert_pem === 'string' ? ca.json.ca_cert_pem : ''
    if (pem === '') {
      host = { ...host, error: `取服务端 CA 失败：${describeFailure(ca)}` }
      return
    }
    try { writeFileSync(caPath, pem, { mode: 0o600 }) } catch { /* 落盘失败下面 spawn 会报 */ }
  }
  const args = [
    '--serve', settings.clip,
    '--source', settings.hostSource,
    '--signal-server', settings.server,
    '--signal-device-key', settings.hostDeviceKey,
    '--signal-ca', caPath,
    '--signal-cert-dir', join(TMP, 'dsh-rabbitcontrol-host-certs'),
    '--duration', '3600s',
    '--signal-overall', '3600s',
  ]
  const child = spawn(settings.bridge, args, {
    stdio: ['ignore', 'pipe', 'pipe'],
    env: { ...process.env, RC_SIGNAL_TOKEN: token },
  })
  host = { child, deviceId: '', backend: '', error: '', restarts: host.restarts + 1, log: [] }
  const remember = (line) => {
    host.log = [...host.log, line].slice(-20)
    if (/被控端错误：|error=Some\(/.test(line)) host.error = line.slice(0, 300)
  }
  const consume = (chunk) => {
    for (const line of String(chunk).split('\n')) {
      const text = line.trim()
      if (text === '') continue
      remember(text)
      const registered = /已登记 [0-9 ]*（(\d{9})）/.exec(text)
      if (registered !== null) host.deviceId = registered[1]
      const backend = /后端=([a-z+]+)/.exec(text)
      if (backend !== null) host.backend = backend[1]
    }
  }
  child.stdout.on('data', consume)
  child.stderr.on('data', consume)
  child.on('exit', (code) => {
    if (host.child === child) host = { ...host, child: null, error: host.error === '' ? `被控端退出（code=${String(code)}）` : host.error }
    // 只要开关还开着就自动拉起：被控端可能因为一轮 accept 超时等原因退出。
    if (settings.hostMode) setTimeout(() => { void startHost(ctx) }, 2000)
  })
  console.error(`[rabbitcontrol-client] 被控端已启动（${settings.hostSource}，key=${settings.hostDeviceKey}）`)
}

/** 停止被控端。 */
function stopHost() {
  const child = host.child
  host = { ...host, child: null, deviceId: '', backend: '' }
  if (child !== null) {
    try { child.kill() } catch { /* 已经退出 */ }
  }
}

/**
 * 被控端状态 / 开关路由。
 */
async function hostHandler(ctx, req, res) {
  if (!fence(ctx, req, res)) return
  if (req.method === 'GET') {
    res.writeHead(200, { 'content-type': 'application/json' })
    res.end(JSON.stringify({
      enabled: settings.hostMode,
      running: host.child !== null,
      deviceId: host.deviceId,
      backend: host.backend,
      error: host.error,
      source: settings.hostSource,
      platform: `${process.platform}-${process.arch}`,
      requiresWindowsGui: process.platform === 'win32' && settings.hostSource === 'real'
        && (settings.bridge === '' || resolve(settings.bridge).toLowerCase() === join(BUNDLE_DIR, 'bin', 'rc-bridge-win-x64.exe').toLowerCase()),
      log: host.log.slice(-8),
    }))
    return
  }
  if (req.method !== 'POST') {
    res.writeHead(405, { allow: 'GET, POST' })
    res.end('method not allowed')
    return
  }
  let body
  try {
    body = JSON.parse(await readBody(req, 8 * 1024))
  } catch (error) {
    res.writeHead(400, { 'content-type': 'text/plain; charset=utf-8' })
    res.end(`请求体不是合法 JSON：${error instanceof Error ? error.message : String(error)}`)
    return
  }
  const enabled = body?.enabled === true
  if (enabled === settings.hostMode && (enabled === false || host.child !== null)) {
    res.writeHead(200, { 'content-type': 'application/json' })
    res.end(JSON.stringify({ enabled: settings.hostMode, running: host.child !== null, deviceId: host.deviceId, backend: host.backend, error: host.error }))
    return
  }
  settings = { ...settings, hostMode: enabled }
  if (enabled) {
    host.error = ''
    await startHost(ctx)
  } else {
    stopHost()
  }
  res.writeHead(200, { 'content-type': 'application/json' })
  res.end(JSON.stringify({ enabled: settings.hostMode, running: host.child !== null, deviceId: host.deviceId, backend: host.backend, error: host.error }))
}

/**
 * 过框架围栏：面板是同源带凭据的页面，未带凭据的请求一律拒绝。
 * @param ctx - Host 上下文。
 * @param req - Node 请求。
 * @param res - Node 响应。
 * @returns 是否放行。
 */
function fence(ctx, req, res) {
  let connection
  try { connection = ctx.get('connection') } catch { connection = undefined }
  if (connection === undefined) {
    res.writeHead(503, { 'content-type': 'text/plain' })
    res.end('no connection service')
    return false
  }
  const rejection = connection.requestRejection(req)
  if (rejection !== undefined) {
    res.writeHead(rejection)
    res.end(rejection === 401 ? 'unauthorized' : 'forbidden')
    return false
  }
  return true
}

/**
 * 归一化服务器地址：只接受 http/https，去掉尾部斜杠与路径。
 * @param value - 原始输入。
 * @returns `scheme://host[:port]`，或 undefined（不合法）。
 */
function normalizeServer(value) {
  if (typeof value !== 'string') return undefined
  try {
    const url = new URL(value.trim())
    if (url.protocol !== 'http:' && url.protocol !== 'https:') return undefined
    return `${url.protocol}//${url.host}`
  } catch {
    return undefined
  }
}

/**
 * 读共享的 rc-server 会话记录（不刷新）。
 * @param ctx - Host 上下文。
 * @returns 会话对象，或 undefined。
 */
async function readSession(ctx) {
  let credentials
  try { credentials = ctx.get('credentials') } catch { credentials = undefined }
  if (credentials === undefined) return undefined
  try {
    const record = await credentials.readRecord(CREDENTIAL_KEY)
    return record?.kind === 'grant' ? record.payload : undefined
  } catch {
    return undefined
  }
}

/**
 * 写 / 清共享的 rc-server 会话记录（与「服务器状态」面板共用同一条）。
 * @param ctx - Host 上下文。
 * @param next - 会话对象；null/undefined 表示清除。
 * @returns 是否写入成功。
 */
async function saveSession(ctx, next) {
  let credentials
  try { credentials = ctx.get('credentials') } catch { credentials = undefined }
  if (credentials === undefined) return false
  try {
    if (next === null || next === undefined) await credentials.deleteRecord(CREDENTIAL_KEY)
    else await credentials.modifyRecord(CREDENTIAL_KEY, async () => ({ kind: 'grant', payload: { ...next } }))
    return true
  } catch {
    return false
  }
}

/**
 * 未配置地址 / 未登录时给出可读拒绝，供路由共用。
 * @param ctx - Host 上下文。
 * @param res - Node 响应。
 * @returns 可用 token；不可用时已写响应并返回 undefined。
 */
async function requireServerAndToken(ctx, res) {
  if (settings.server === '') {
    res.writeHead(409, { 'content-type': 'text/plain; charset=utf-8' })
    res.end('还没有配置 rc-server 地址：请在面板的「服务器」里填写地址并登录')
    return undefined
  }
  const token = await accessToken(ctx)
  if (token === undefined) {
    res.writeHead(409, { 'content-type': 'text/plain; charset=utf-8' })
    res.end('需要先登录 rc-server：请在面板的「服务器」里用账号登录')
    return undefined
  }
  return token
}

/**
 * 会话状态路由：面板据此决定显示设置卡片还是设备列表。
 */
async function sessionHandler(ctx, req, res) {
  if (!fence(ctx, req, res)) return
  if (req.method !== 'GET') {
    res.writeHead(405, { allow: 'GET' })
    res.end('method not allowed')
    return
  }
  let session = await readSession(ctx)
  // 有 token 不等于被**这台**服务器接受（换服务器后旧 token 会被 401）：
  // 用 /api/v1/me 验一次，失效就清掉，让面板回到「登录」状态。
  if (typeof session?.accessToken === 'string' && settings.server !== '') {
    const me = await rcRequest('/api/v1/me', { token: session.accessToken })
    if (me.status === 401) {
      await saveSession(ctx, null)
      session = undefined
    }
  }
  res.writeHead(200, { 'content-type': 'application/json' })
  res.end(JSON.stringify({
    server: settings.server,
    authorized: typeof session?.accessToken === 'string',
    email: typeof session?.email === 'string' ? session.email : '',
    signalKey: settings.signalDeviceKey,
  }))
}

/**
 * 设置服务器地址：写进插件配置（写不进就只在本进程生效并如实说明），并清掉旧会话。
 */
async function serverHandler(ctx, req, res) {
  if (!fence(ctx, req, res)) return
  if (req.method !== 'POST') {
    res.writeHead(405, { allow: 'POST' })
    res.end('method not allowed')
    return
  }
  let body
  try {
    body = JSON.parse(await readBody(req, 8 * 1024))
  } catch (error) {
    res.writeHead(400, { 'content-type': 'text/plain; charset=utf-8' })
    res.end(`请求体不是合法 JSON：${error instanceof Error ? error.message : String(error)}`)
    return
  }
  const next = normalizeServer(body?.server)
  if (next === undefined) {
    res.writeHead(400, { 'content-type': 'text/plain; charset=utf-8' })
    res.end('地址不合法：需要 http(s)://主机[:端口]')
    return
  }
  let saved = false
  let note = '已在本进程内生效；没有可写的配置入口，重启后会恢复原值'
  const editor = ctx.get('configEditor')
  const entry = ctx.fiber?.entry
  if (editor !== undefined && entry !== undefined) {
    try {
      await editor.edit(entry, (current) => ({ ...current, serverUrl: next }))
      saved = true
      note = '已写入插件配置'
    } catch (error) {
      note = `未能写入插件配置：${error instanceof Error ? error.message : String(error)}`
    }
  }
  settings = { ...settings, server: next }
  // 换服务器后旧 token 无效，必须重新登录。
  await saveSession(ctx, null)
  res.writeHead(200, { 'content-type': 'application/json' })
  res.end(JSON.stringify({ server: next, saved, note, authorized: false }))
}

/**
 * 登录：用 rc-server 账号换 token，写进与「服务器状态」面板共享的凭据记录。
 */
async function loginHandler(ctx, req, res) {
  if (!fence(ctx, req, res)) return
  if (req.method !== 'POST') {
    res.writeHead(405, { allow: 'POST' })
    res.end('method not allowed')
    return
  }
  if (settings.server === '') {
    res.writeHead(409, { 'content-type': 'text/plain; charset=utf-8' })
    res.end('还没有配置 rc-server 地址')
    return
  }
  let body
  try {
    body = JSON.parse(await readBody(req, 16 * 1024))
  } catch (error) {
    res.writeHead(400, { 'content-type': 'text/plain; charset=utf-8' })
    res.end(`请求体不是合法 JSON：${error instanceof Error ? error.message : String(error)}`)
    return
  }
  const email = typeof body?.email === 'string' ? body.email.trim() : ''
  const password = typeof body?.password === 'string' ? body.password : ''
  if (email === '' || password === '') {
    res.writeHead(400, { 'content-type': 'text/plain; charset=utf-8' })
    res.end('需要邮箱与口令')
    return
  }
  const result = await rcRequest('/api/v1/auth/login', { method: 'POST', body: { email, password } })
  if (result.status !== 200 || typeof result.json?.access_token !== 'string') {
    res.writeHead(401, { 'content-type': 'text/plain; charset=utf-8' })
    res.end(describeFailure(result))
    return
  }
  const next = {
    accessToken: result.json.access_token,
    refreshToken: typeof result.json.refresh_token === 'string' ? result.json.refresh_token : '',
    email: result.json.user?.email ?? email,
    role: result.json.user?.role ?? 'user',
    expiresAt: Date.now() + (typeof result.json.expires_in_sec === 'number' ? result.json.expires_in_sec : 900) * 1000,
  }
  if (result.json.user?.must_change_password === true) {
    // 受限 token 除了改密什么都做不了；存了只会让面板进入「已登录但处处 403」。
    res.writeHead(200, { 'content-type': 'application/json' })
    res.end(JSON.stringify({ authorized: false, mustChangePassword: true, email: next.email }))
    return
  }
  const stored = await saveSession(ctx, next)
  // 「作为被控端」开着但之前因为没有有效登录没起来：登录成功后补上。
  if (settings.hostMode && host.child === null) void startHost(ctx)
  res.writeHead(200, { 'content-type': 'application/json' })
  res.end(JSON.stringify({ authorized: true, email: next.email, stored, mustChangePassword: false }))
}

/**
 * 退出登录：清掉共享凭据记录。
 */
async function logoutHandler(ctx, req, res) {
  if (!fence(ctx, req, res)) return
  if (req.method !== 'POST') {
    res.writeHead(405, { allow: 'POST' })
    res.end('method not allowed')
    return
  }
  await saveSession(ctx, null)
  res.writeHead(200, { 'content-type': 'application/json' })
  res.end(JSON.stringify({ authorized: false }))
}

/**
 * 取整句失败原因。
 * @param res - rcRequest 的结果。
 * @returns 可展示的原因。
 */
function describeFailure(res) {
  if (res === undefined) return '没有响应'
  if (typeof res.transportError === 'string' && res.transportError !== '') return res.transportError
  if (typeof res.json?.message === 'string' && res.json.message !== '') return res.json.message
  if (typeof res.json?.error === 'string' && res.json.error !== '') return res.json.error
  const text = typeof res.text === 'string' ? res.text.trim() : ''
  return text === '' ? `HTTP ${res.status}` : text.slice(0, 200)
}

/**
 * 取得可用的 access token：优先用凭据记录，过期就用 refresh token 换。
 * @param ctx - Host 上下文。
 * @returns token，或 undefined（未登录 / 已失效）。
 */
async function accessToken(ctx, server = settings.server) {
  let credentials
  try { credentials = ctx.get('credentials') } catch { credentials = undefined }
  if (credentials === undefined) return undefined
  let record
  try { record = await credentials.readRecord(CREDENTIAL_KEY) } catch { return undefined }
  const session = record?.kind === 'grant' ? record.payload : undefined
  if (session === undefined || typeof session.accessToken !== 'string') return undefined
  const fresh = typeof session.expiresAt === 'number' && session.expiresAt - Date.now() > 30_000
  if (fresh) return session.accessToken
  if (typeof session.refreshToken !== 'string' || session.refreshToken === '') return undefined
  if (server !== settings.server) return undefined
  const refreshed = await rcRequest('/api/v1/auth/refresh', { server, method: 'POST', body: { refresh_token: session.refreshToken } })
  if (refreshed.status !== 200 || typeof refreshed.json?.access_token !== 'string') return undefined
  const next = {
    accessToken: refreshed.json.access_token,
    refreshToken: typeof refreshed.json.refresh_token === 'string' ? refreshed.json.refresh_token : session.refreshToken,
    email: refreshed.json.user?.email ?? session.email,
    role: refreshed.json.user?.role ?? session.role,
    expiresAt: Date.now() + (typeof refreshed.json.expires_in_sec === 'number' ? refreshed.json.expires_in_sec : 900) * 1000,
  }
  try {
    const record = await credentials.modifyRecord(CREDENTIAL_KEY, async current =>
      server === settings.server && current?.payload?.refreshToken === session.refreshToken ? { kind: 'grant', payload: next } : undefined)
    if (server !== settings.server || record?.payload?.accessToken !== next.accessToken) return undefined
  } catch { if (server !== settings.server) return undefined }
  return next.accessToken
}

// Older servers can lack the account endpoint. Their global directory requires an
// administrator, and rows without a verified owner must never reach this panel.
async function accountDeviceList(options) {
  const own = await rcRequest('/api/v1/me/devices', options)
  if (own.status !== 404 && own.status !== 405) return own
  const me = await rcRequest('/api/v1/me', options)
  if (me.status !== 200) return me
  if (me.json?.role !== 'admin' || me.json.must_change_password === true || typeof me.json.id !== 'string' || !me.json.id) {
    return { status: 403, json: { message: '此服务器缺少同账号设备列表接口，请升级 rc-server。' } }
  }
  const legacy = await rcRequest(`/api/v1/devices?limit=${MAX_DEVICES}`, options)
  if (legacy.status !== 200 || !Array.isArray(legacy.json?.devices)) return legacy
  return { ...legacy, legacy: true, json: { devices: legacy.json.devices.filter(row => row?.owner_user_id === me.json.id) } }
}

/**
 * 设备列表处理器。
 *
 * 列表来自 rc-server；每个设备带上它登记时公布的 `lan_addrs`——面板据此做局域网直连。
 * 向面板提供设备地址与被控权限。
 * @param ctx - Host 上下文。
 * @param req - Node 请求。
 * @param res - Node 响应。
 */
async function devicesHandler(ctx, req, res) {
  let connection
  try { connection = ctx.get('connection') } catch { connection = undefined }
  if (connection === undefined) {
    res.writeHead(503, { 'content-type': 'text/plain' })
    res.end('no connection service')
    return
  }
  const rejection = connection.requestRejection(req)
  if (rejection !== undefined) {
    res.writeHead(rejection)
    res.end(rejection === 401 ? 'unauthorized' : 'forbidden')
    return
  }
  if (req.method !== 'GET') {
    res.writeHead(405, { allow: 'GET' })
    res.end('method not allowed')
    return
  }
  const reply = (value) => {
    const body = Buffer.from(JSON.stringify(value), 'utf8')
    res.writeHead(200, { 'content-type': 'application/json; charset=utf-8', 'content-length': String(body.length), 'cache-control': 'no-store' })
    res.end(body)
  }
  try {
    const server = settings.server
    const token = await accessToken(ctx, server)
    if (token === undefined || server !== settings.server) {
      reply({ authorized: false, server: settings.server, devices: [] })
      return
    }
    const result = await accountDeviceList({ token, server })
    if (result.status === 401 || server !== settings.server) {
      reply({ authorized: false, server: settings.server, devices: [] })
      return
    }
    if (result.status !== 200) {
      reply({ authorized: true, server: settings.server, devices: [], error: describeFailure(result) })
      return
    }
    if (!Array.isArray(result.json?.devices)) {
      reply({ authorized: true, server: settings.server, devices: [], error: '服务器返回了无法识别的设备列表。' })
      return
    }
    const rows = result.json.devices
    // 被管理端禁用的设备不列出来：它们连不上，列着只会让人点空。
    const usable = rows.filter((row) => row && row.disabled !== true)
    usable.sort((a, b) => Number(b.online === true) - Number(a.online === true))
    reply({
      authorized: true,
      server: settings.server,
      legacy: result.legacy === true,
      devices: usable.slice(0, MAX_DEVICES).map((row) => ({
        deviceId: row?.device_id,
        alias: row?.alias ?? '',
        hostname: row?.hostname ?? '',
        os: row?.os ?? '',
        online: row?.online === true,
        inSession: row?.in_session === true,
        allowRemoteControl: row.allow_remote_control ?? !/android|ios/i.test(row.os ?? ''),
        wolCapable: !result.legacy && row.wol_capable === true,
        canWake: !result.legacy && row.wol_capable === true && row.online === false && row.in_session !== true
          && Boolean(settings.bridge) && settings.server.startsWith('https://') && SIGNAL_DEVICE_ID.test(row.device_id),
        lanAddrs: Array.isArray(row?.lan_addrs) ? row.lan_addrs.map(normalizeAddress).filter(Boolean) : [],
      })),
    })
  } catch (error) {
    reply({ authorized: false, server: settings.server, devices: [], error: error instanceof Error ? error.message : String(error) })
  }
}

let wakeBusy = false

async function wakeHandler(ctx, req, res) {
  if (!fence(ctx, req, res)) return
  if (req.method !== 'POST') {
    res.writeHead(405, { allow: 'POST' }).end()
    return
  }
  const reply = (status, code, method) => {
    res.writeHead(status, { 'content-type': 'application/json', 'cache-control': 'no-store' })
    res.end(JSON.stringify({ ok: status === 200, code, method }))
  }
  if (req.headers['content-type']?.split(';')[0] !== 'application/json') return reply(415, 'invalid_input')
  let target
  try { target = JSON.parse(await readBody(req, 1024)).deviceId } catch { return reply(400, 'invalid_input') }
  if (typeof target !== 'string' || !SIGNAL_DEVICE_ID.test(target)) return reply(400, 'invalid_input')
  if (wakeBusy) return reply(409, 'wake_busy')
  const config = settings
  const { server, bridge } = config
  if (!server.startsWith('https://') || !bridge) return reply(409, 'bridge_unavailable')
  wakeBusy = true
  let directory
  try {
    const token = await accessToken(ctx)
    if (!token || settings !== config) return reply(401, 'sign_in_required')
    const options = { server, token }
    const own = await rcRequest('/api/v1/me/devices', options)
    if (own.status !== 200) return reply(own.status || 502, own.status === 401 ? 'sign_in_required' : own.status === 403 ? 'forbidden' : 'account_list_unavailable')
    const device = own.json?.devices?.find(row => row?.device_id === target)
    if (!device || device.disabled === true) return reply(403, 'forbidden')
    if (device.online !== false || device.in_session === true) return reply(409, 'already_online')
    if (device.wol_capable !== true || target === host.deviceId) return reply(409, 'wol_disabled')
    const me = await rcRequest('/api/v1/me', options)
    if (me.status !== 200 || typeof me.json?.id !== 'string' || !me.json.id) return reply(403, 'forbidden')
    const scope = createHash('sha256').update(JSON.stringify([server, me.json.id])).digest('hex')
    const identity = await ctx.get('credentials').modifyRecord(`dsh-rabbitcontrol/wake-${scope}`, async current =>
      current?.kind === 'api-key' && /^[0-9a-f]{64}$/.test(current.key) ? undefined : { kind: 'api-key', key: randomBytes(32).toString('hex') })
    if (!identity?.key) return reply(503, 'bridge_unavailable')
    const ca = await rcRequest('/api/v1/pki/ca', { server })
    if (ca.status !== 200 || typeof ca.json?.ca_cert_pem !== 'string' || !ca.json.ca_cert_pem) return reply(502, 'ca_unavailable')
    directory = await mkdtemp(join(TMP, 'rc-wake-'))
    const caPath = join(directory, 'ca.pem')
    await writeFile(caPath, ca.json.ca_cert_pem, { mode: 0o600 })
    if (settings !== config || (await readSession(ctx))?.accessToken !== token) return reply(409, 'session_changed')
    const args = ['--signal-wake', '--signal-target', target, '--signal-server', server, '--signal-device-key', identity.key,
      '--signal-ca', caPath, '--signal-wait', '8s']
    let output
    let failed = false
    try {
      output = await runBridge(bridge, args, { env: { ...process.env, RC_SIGNAL_TOKEN: token }, timeout: 20000, killSignal: 'SIGKILL', maxBuffer: 65536, windowsHide: true })
    } catch (error) {
      if (error.killed) return reply(504, 'wake_timeout')
      if (error.code !== 1 || !error.stdout?.trim()) return reply(502, 'bridge_failed')
      output = error
      failed = true
    }
    let ack
    try { ack = JSON.parse(output.stdout.trim()) } catch { return reply(502, 'invalid_ack') }
    if (ack?.event !== 'wol_ack' || ack.targetId !== target || typeof ack.ok !== 'boolean' || ack.ok === failed) return reply(502, 'invalid_ack')
    if (!ack.ok) return reply(409, typeof ack.code === 'string' && /^[a-z_]{1,64}$/.test(ack.code) ? ack.code : 'wake_failed')
    if (!['server', 'lan_peer', 'already_online'].includes(ack.method)) return reply(502, 'invalid_ack')
    return reply(200, '', ack.method)
  } catch {
    return reply(502, 'bridge_failed')
  } finally {
    if (directory) await rm(directory, { recursive: true, force: true }).catch(() => {})
    wakeBusy = false
  }
}

/**
 * 客户端报错路由：把浏览器侧上报的文本追加到 `/tmp/dsh-rabbitcontrol-client.log`。
 * @param ctx - Host 上下文。
 * @param req - Node 请求。
 * @param res - Node 响应。
 */
async function reportHandler(ctx, req, res) {
  let connection
  try { connection = ctx.get('connection') } catch { connection = undefined }
  if (connection === undefined) {
    res.writeHead(503, { 'content-type': 'text/plain' })
    res.end('no connection service')
    return
  }
  const rejection = connection.requestRejection(req)
  if (rejection !== undefined) {
    res.writeHead(rejection)
    res.end(rejection === 401 ? 'unauthorized' : 'forbidden')
    return
  }
  if (req.method !== 'POST') {
    res.writeHead(405, { allow: 'POST' })
    res.end('method not allowed')
    return
  }
  try {
    const raw = await readBody(req, 16 * 1024)
    appendFileSync(REPORT_FILE, `${new Date().toISOString()} ${raw}\n`)
    res.writeHead(204)
    res.end()
  } catch (error) {
    res.writeHead(400, { 'content-type': 'text/plain' })
    res.end(error instanceof Error ? error.message : String(error))
  }
}

/**
 * 输入路由处理器：POST `{ events: [...] }` 写进当前会话的 stdin。
 * @param ctx - Host 上下文。
 * @param req - Node 请求。
 * @param res - Node 响应。
 */
async function inputHandler(ctx, req, res) {
  let connection
  try {
    connection = ctx.get('connection')
  } catch {
    connection = undefined
  }
  if (connection === undefined) {
    res.writeHead(503, { 'content-type': 'text/plain' })
    res.end('no connection service')
    return
  }
  const rejection = connection.requestRejection(req)
  if (rejection !== undefined) {
    res.writeHead(rejection)
    res.end(rejection === 401 ? 'unauthorized' : 'forbidden')
    return
  }
  if (req.method !== 'POST') {
    res.writeHead(405, { allow: 'POST' })
    res.end('method not allowed')
    return
  }
  try {
    const raw = await readBody(req)
    let parsed
    try {
      parsed = raw === '' ? {} : JSON.parse(raw)
    } catch {
      res.writeHead(400, { 'content-type': 'application/json' })
      res.end('{"error":"invalid_json"}')
      return
    }
    if (!Array.isArray(parsed.events) || parsed.events.length === 0) {
      res.writeHead(400, { 'content-type': 'application/json' })
      res.end('{"error":"empty_events"}')
      return
    }
    const failure = writeInput({ events: parsed.events })
    if (failure !== undefined) {
      res.writeHead(failure === 'too_large' ? 413 : 409, { 'content-type': 'application/json' })
      res.end(JSON.stringify({ error: failure }))
      return
    }
    res.writeHead(204)
    res.end()
  } catch (error) {
    res.writeHead(400, { 'content-type': 'application/json' })
    res.end(JSON.stringify({ error: error instanceof Error ? error.message : String(error) }))
  }
}

export function apply(ctx, config = {}) {
  settings = {
    bridge: typeof config.bridgePath === 'string' && config.bridgePath !== '' ? config.bridgePath : defaultBridge(),
    clip: typeof config.clipPath === 'string' && config.clipPath !== '' ? config.clipPath : DEFAULT_CLIP,
    server: typeof config.serverUrl === 'string' && config.serverUrl !== '' ? config.serverUrl : DEFAULT_SERVER,
    signalDeviceKey: typeof config.signalDeviceKey === 'string' && config.signalDeviceKey !== ''
      ? config.signalDeviceKey
      : SIGNAL_DEVICE_KEY,
    signalTransport: ['auto', 'p2p', 'relay'].includes(config.signalTransport)
      ? config.signalTransport
      : 'auto',
    insecureTls: config.insecureTls !== false,
    hostMode: config.hostMode === true,
    hostSource: config.hostSource === 'clip' ? 'clip' : 'real',
    hostDeviceKey: typeof config.hostDeviceKey === 'string' && config.hostDeviceKey !== ''
      ? config.hostDeviceKey
      : HOST_DEVICE_KEY,
  }
  ctx.effect(
    () => ctx.webServer.register({
      kind: 'exact',
      path: STREAM_PATH,
      handler: (req, res) => { routeHandler(ctx, req, res) },
    }),
    'rabbitcontrol-client: stream route',
  )
  ctx.effect(
    () => ctx.webServer.register({
      kind: 'exact',
      path: HOST_PATH,
      handler: (req, res) => { void hostHandler(ctx, req, res) },
    }),
    'rabbitcontrol-client: host route',
  )
  ctx.effect(
    () => ctx.webServer.register({
      kind: 'exact',
      path: SESSION_PATH,
      handler: (req, res) => { void sessionHandler(ctx, req, res) },
    }),
    'rabbitcontrol-client: session route',
  )
  ctx.effect(
    () => ctx.webServer.register({
      kind: 'exact',
      path: SERVER_PATH,
      handler: (req, res) => { void serverHandler(ctx, req, res) },
    }),
    'rabbitcontrol-client: server route',
  )
  ctx.effect(
    () => ctx.webServer.register({
      kind: 'exact',
      path: LOGIN_PATH,
      handler: (req, res) => { void loginHandler(ctx, req, res) },
    }),
    'rabbitcontrol-client: login route',
  )
  ctx.effect(
    () => ctx.webServer.register({
      kind: 'exact',
      path: LOGOUT_PATH,
      handler: (req, res) => { void logoutHandler(ctx, req, res) },
    }),
    'rabbitcontrol-client: logout route',
  )
  ctx.effect(
    () => ctx.webServer.register({
      kind: 'exact',
      path: REPORT_PATH,
      handler: (req, res) => { void reportHandler(ctx, req, res) },
    }),
    'rabbitcontrol-client: report route',
  )
  ctx.effect(() => ctx.webServer.register({
    kind: 'exact', path: '/rabbitcontrol-client/wake', handler: (req, res) => wakeHandler(ctx, req, res),
  }), 'rabbitcontrol-client: wake route')
  ctx.effect(
    () => ctx.webServer.register({
      kind: 'exact',
      path: DEVICES_PATH,
      handler: (req, res) => { void devicesHandler(ctx, req, res) },
    }),
    'rabbitcontrol-client: devices route',
  )
  ctx.effect(
    () => ctx.webServer.register({
      kind: 'exact',
      path: INPUT_PATH,
      handler: (req, res) => { void inputHandler(ctx, req, res) },
    }),
    'rabbitcontrol-client: input route',
  )
  ctx.effect(() => {
    if (settings.hostMode) void startHost(ctx)
    return () => {
      settings = { ...settings, hostMode: false }
      stopHost()
    }
  }, 'rabbitcontrol-client: host supervisor')
}

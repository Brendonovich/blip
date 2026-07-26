import * as Effect from "effect/Effect"
import * as Layer from "effect/Layer"
import * as Option from "effect/Option"
import * as Redacted from "effect/Redacted"
import * as HttpEffect from "effect/unstable/http/HttpEffect"
import * as HttpRouter from "effect/unstable/http/HttpRouter"
import * as HttpServerRequest from "effect/unstable/http/HttpServerRequest"
import * as HttpServerResponse from "effect/unstable/http/HttpServerResponse"
import * as HttpApiBuilder from "effect/unstable/httpapi/HttpApiBuilder"
import * as HttpApiScalar from "effect/unstable/httpapi/HttpApiScalar"
import {
  Auth as AuthModule,
  BlipApi,
  CurrentUser as CurrentUserModule,
  InternalError,
  InvalidRequest,
  NotFound,
  ObjectStorage as ObjectStorageModule,
  PasswordRequired,
  Repository as RepositoryModule,
  SetupAuth,
  type StorageConfig,
  StorageUnavailable,
  Unauthorized,
  UserAuth,
  Vault as VaultModule,
  VerifiedPasswordHashes as VerifiedPasswordHashesModule,
  type VideoPrivacy,
  type VideoRecord
} from "@blip/server-domain"

const Auth = AuthModule.Service
const CurrentUser = CurrentUserModule.Service
const ObjectStorage = ObjectStorageModule.Service
const Repository = RepositoryModule.Service
const Vault = VaultModule.Service
const VerifiedPasswordHashes = VerifiedPasswordHashesModule.Service

const PART_SIZE = 8 * 1024 * 1024
const MAX_RECORDING_SIZE = PART_SIZE * 10_000

const landingPage = `<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Blip server</title><style>body{font:16px system-ui;max-width:44rem;margin:12vh auto;padding:0 1.5rem;color:#18181b}code{background:#f4f4f5;padding:.15rem .35rem;border-radius:.3rem}</style></head>
<body><h1>Blip server</h1><p>This server is ready. Provision a user with <code>POST /api/users</code>, connect that user's S3-compatible bucket with <code>PUT /api/storage</code>, then add <code>https://this-host#user-api-key</code> to Blip Capture.</p><p>The typed API description is available at <code>/openapi.json</code>.</p></body></html>`

const jsonForHtml = (value: unknown) => JSON.stringify(value).replaceAll("<", "\\u003c")

const videoPage = (options: {
  readonly id: string
  readonly privacy: VideoPrivacy
  readonly owner: boolean
  readonly source: string | undefined
}) => `<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>Blip recording</title>
<style>
:root{color:#e8e6dc;background:#11110f;font:12px "SFMono-Regular",Consolas,monospace}*{box-sizing:border-box}body{margin:0;min-width:320px;min-height:100vh;background:radial-gradient(circle at 50% 25%,#282722 0,#11110f 55%)}button,input,select{font:inherit}.shell{min-height:100vh;display:grid;grid-template-rows:auto 1fr auto;padding:0 32px}.top{height:72px;display:flex;align-items:center;justify-content:space-between;border-bottom:1px solid #3b3a35}.brand{display:flex;align-items:center;gap:12px;color:inherit;text-decoration:none;letter-spacing:.18em}.signal{width:8px;height:8px;border-radius:50%;background:#e8532f;box-shadow:0 0 0 4px #e8532f22}.access{color:#918e84;font-size:9px;letter-spacing:.13em;text-transform:uppercase}.stage{display:grid;place-items:center;padding:42px 0}.frame{width:min(1180px,100%);aspect-ratio:16/9;background:#080807;border:1px solid #45433d;box-shadow:0 28px 80px #0008;display:grid;place-items:center;overflow:hidden}video{width:100%;height:100%;object-fit:contain}.gate{width:min(390px,calc(100% - 40px));text-align:center}.lock{width:44px;height:44px;margin:0 auto 24px;border:1px solid #5f5c53;border-radius:50%;display:grid;place-items:center;color:#e8532f;font-size:17px}.gate h1{margin:0 0 12px;font:400 34px "Iowan Old Style",Baskerville,serif}.gate p{margin:0 0 26px;color:#918e84;line-height:1.7}.gate form{display:grid;grid-template-columns:1fr auto;border-bottom:1px solid #77736a}.gate input{min-width:0;padding:13px 0;color:inherit;background:none;border:0;outline:0}.gate button,.save{padding:12px 17px;border:0;background:#e8e6dc;color:#171714;cursor:pointer;text-transform:uppercase;font-size:9px;letter-spacing:.09em}.error{display:block;min-height:16px;margin-top:13px;color:#e88167;font-size:9px}.footer{min-height:78px;display:flex;align-items:center;justify-content:space-between;gap:24px;border-top:1px solid #3b3a35;color:#77746b}.owner{display:flex;align-items:center;gap:12px}.owner label{font-size:9px;letter-spacing:.1em;text-transform:uppercase}.owner select,.owner input{height:36px;border:1px solid #4c4a44;background:#1b1b18;color:#e8e6dc;padding:0 10px;outline:0}.owner input{width:160px}.save{background:#e8532f;color:white}.status{font-size:9px;color:#b6b2a7}@media(max-width:700px){.shell{padding:0 16px}.stage{padding:20px 0}.frame{aspect-ratio:auto;min-height:54vh}.footer{padding:18px 0;align-items:flex-start;flex-direction:column}.owner{width:100%;flex-wrap:wrap}.owner input{flex:1}.access{display:none}}
/* Match the admin shell while keeping the player bounded by the viewport. */
:root{color:#24231e;background:#eeede5;font-synthesis:none}body{background:#eeede5}.shell{width:100%;max-width:1500px;height:100dvh;min-height:0;margin:auto;padding:0 42px;grid-template-rows:auto minmax(0,1fr) auto}.top{height:86px;border-color:#bbb9ae}.brand{gap:14px;font-weight:500}.signal{width:9px;height:9px;box-shadow:0 0 0 4px rgb(232 83 47 / 14%)}.access{color:#747168;font-size:10px}.stage{min-height:0;padding:42px 0}.frame{border-color:#bbb9ae;box-shadow:none}.frame:has(video){width:100%;height:100%;min-height:0;aspect-ratio:auto;background:transparent;border:0}.frame:not(:has(video)){width:100%;height:100%;min-height:0;aspect-ratio:auto;background:transparent;border:0;box-shadow:none}video{display:block;background:transparent}.lock{width:auto;height:auto;margin-bottom:22px;border:0;border-radius:0;font-size:0;letter-spacing:.2em}.lock::after{content:"RECORDING ACCESS";font-size:10px;font-weight:500}.gate h1{color:#24231e}.gate p{color:#65635b}.gate form{border-color:#aaa89e}.gate form:has(input:focus){border-color:#e8532f}.gate input{color:#24231e}.gate button,.save{border:1px solid #24231e;background:#24231e;color:#f4f2e9;padding:13px 18px;font-size:10px;letter-spacing:.08em}.gate button:hover,.save:hover{background:#e8532f;border-color:#e8532f}.error,.status{color:#8e4c3e}.footer{min-height:78px;border-color:#bbb9ae;color:#858278;font-size:8px;letter-spacing:.14em}.owner label{color:#747168}.owner select,.owner input{height:40px;border:0;border-bottom:1px solid #aaa89e;border-radius:0;background:transparent;color:#24231e;outline:0}.owner select:focus,.owner input:focus{border-color:#e8532f}.owner input{padding:0}.save{height:40px}.status{letter-spacing:0}@media(max-width:800px){.shell{padding-left:20px;padding-right:20px}.stage{padding:20px 0}.access{display:none}.footer{padding:18px 0;align-items:flex-start;flex-direction:column}.owner{width:100%;flex-wrap:wrap}.owner input{flex:1}}
</style></head>
<body><main class="shell"><header class="top"><a class="brand" href="/"><span class="signal"></span>BLIP</a><span class="access">${options.owner ? "Owner preview" : options.privacy === "public" ? "Public recording" : "Protected recording"}</span></header><section class="stage"><div class="frame" id="frame">${options.source ? `<video controls autoplay playsinline src=${jsonForHtml(options.source)}></video>` : options.privacy === "password" ? `<div class="gate"><div class="lock">+</div><h1>Private recording</h1><p>Enter the password shared by the owner to watch this recording.</p><form id="unlock"><input id="password" type="password" autocomplete="current-password" placeholder="Password" required><button>Watch</button></form><span class="error" id="error"></span></div>` : `<div class="gate"><div class="lock">-</div><h1>Recording unavailable</h1><p>The owner has made this recording private.</p></div>`}</div></section><footer class="footer"><span>REC / ${options.id.slice(0, 8).toUpperCase()}</span>${options.owner ? `<div class="owner"><label for="privacy">Who can view</label><select id="privacy"><option value="public"${options.privacy === "public" ? " selected" : ""}>Anyone with the link</option><option value="password"${options.privacy === "password" ? " selected" : ""}>Anyone with a password</option><option value="private"${options.privacy === "private" ? " selected" : ""}>Only me</option></select><input id="new-password" type="password" placeholder="New password" aria-label="New password"><button class="save" id="save">Save</button><span class="status" id="status"></span></div>` : "<span>SHARED WITH BLIP</span>"}</footer></main>
<script>const id=${jsonForHtml(options.id)};const unlock=document.querySelector('#unlock');unlock?.addEventListener('submit',async(e)=>{e.preventDefault();const error=document.querySelector('#error');error.textContent='Checking...';const response=await fetch('/v/'+encodeURIComponent(id)+'/unlock',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({password:document.querySelector('#password').value})});if(!response.ok){error.textContent=response.status===401?'That password is not correct.':'Could not open recording.';return}const media=await response.json();const source=media.playlist?URL.createObjectURL(new Blob([media.source],{type:'application/vnd.apple.mpegurl'})):media.source;document.querySelector('#frame').innerHTML='<video controls autoplay playsinline></video>';const video=document.querySelector('video');video.src=source;video.play()});const privacy=document.querySelector('#privacy'),password=document.querySelector('#new-password');const sync=()=>{if(password)password.hidden=privacy.value!=='password'};privacy?.addEventListener('change',sync);sync();document.querySelector('#save')?.addEventListener('click',async()=>{const status=document.querySelector('#status');status.textContent='Saving...';const response=await fetch('/api/uploads/'+encodeURIComponent(id)+'/privacy',{method:'PUT',headers:{'content-type':'application/json'},body:JSON.stringify({privacy:privacy.value,password:password.value||undefined})});if(!response.ok){const body=await response.json().catch(()=>({}));status.textContent=body.message||'Could not save';return}location.reload()});</script></body></html>`

const secureEqual = (left: string, right: string) => {
  if (left.length !== right.length) return false
  let difference = 0
  for (let index = 0; index < left.length; index++) {
    difference |= left.charCodeAt(index) ^ right.charCodeAt(index)
  }
  return difference === 0
}

const randomToken = (prefix: string) => {
  const bytes = crypto.getRandomValues(new Uint8Array(32))
  let binary = ""
  for (const byte of bytes) binary += String.fromCharCode(byte)
  const encoded = btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "")
  return `${prefix}${encoded}`
}

const hashApiKey = (apiKey: string) => Effect.promise(async () => {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(apiKey))
  return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, "0")).join("")
})

const encodeBytes = (bytes: Uint8Array) => {
  let binary = ""
  for (const byte of bytes) binary += String.fromCharCode(byte)
  return btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "")
}

const decodeBytes = (value: string) => {
  const standard = value.replaceAll("-", "+").replaceAll("_", "/")
  const binary = atob(standard.padEnd(Math.ceil(standard.length / 4) * 4, "="))
  const bytes = new Uint8Array(binary.length)
  for (let index = 0; index < binary.length; index++) bytes[index] = binary.charCodeAt(index)
  return bytes
}

const derivePassword = async (password: string, salt: Uint8Array<ArrayBuffer>) => {
  const key = await crypto.subtle.importKey("raw", new TextEncoder().encode(password), "PBKDF2", false, ["deriveBits"])
  return new Uint8Array(await crypto.subtle.deriveBits({
    name: "PBKDF2",
    hash: "SHA-256",
    salt,
    iterations: 10_000
  }, key, 256))
}

const hashPassword = (password: string) => Effect.promise(async () => {
  const salt = crypto.getRandomValues(new Uint8Array(16))
  return `v1.${encodeBytes(salt)}.${encodeBytes(await derivePassword(password, salt))}`
})

const verifyPassword = (password: string, encoded: string) => Effect.promise(async () => {
  try {
    const [version, salt, expected] = encoded.split(".")
    if (version !== "v1" || !salt || !expected) return false
    return secureEqual(encodeBytes(await derivePassword(password, decodeBytes(salt))), expected)
  } catch {
    return false
  }
})

const PASSWORD_COOKIE = "x-blip-password"
const MAX_VERIFIED_HASHES = 10

const getVerifiedPasswordHashes = Effect.gen(function*() {
  const service = yield* Effect.serviceOption(VerifiedPasswordHashes)
  if (Option.isSome(service)) {
    return service.value.hashes
  }
  const request = yield* HttpServerRequest.HttpServerRequest
  const cookieValue = request.cookies[PASSWORD_COOKIE]
  if (!cookieValue) return [] as ReadonlyArray<string>
  const vault = yield* Vault
  const decrypted = yield* vault.decryptString(cookieValue).pipe(Effect.catch(() => Effect.succeed("[]")))
  try {
    const parsed: unknown = JSON.parse(decrypted)
    if (Array.isArray(parsed) && parsed.every((hash) => typeof hash === "string")) {
      return parsed as ReadonlyArray<string>
    }
  } catch {}
  return [] as ReadonlyArray<string>
})

const setVerifiedPasswordCookie = (passwordHash: string) => Effect.gen(function*() {
  const hashes = [...(yield* getVerifiedPasswordHashes)].filter((hash) => hash !== passwordHash)
  hashes.push(passwordHash)
  const vault = yield* Vault
  const cookieValue = yield* vault.encryptString(JSON.stringify(hashes.slice(-MAX_VERIFIED_HASHES))).pipe(Effect.orDie)
  const request = yield* HttpServerRequest.HttpServerRequest
  const secure = new URL(request.url).protocol === "https:"
  yield* HttpEffect.appendPreResponseHandler((_req, response) =>
    Effect.succeed(HttpServerResponse.setCookieUnsafe(response, PASSWORD_COOKIE, cookieValue, {
      httpOnly: true,
      secure,
      sameSite: "lax",
      path: "/"
    }))
  )
})

const internal = <A, E extends { readonly _tag: string }, R>(effect: Effect.Effect<A, E, R>) =>
  Effect.mapError(effect, (err: any) => {
    console.error("internal error:", JSON.stringify(err, null, 2))
    const msg = err instanceof Error ? err.message : JSON.stringify(err)
    return new InternalError({ message: `Internal server error: ${msg}` })
  })

const unavailable = <A, E extends { readonly _tag: string }, R>(effect: Effect.Effect<A, E, R>) =>
  effect.pipe(
    Effect.tapError((error) => Effect.sync(() => {
      const cause = "cause" in error ? error.cause : error
      console.error(
        "Object storage operation failed",
        cause instanceof Error ? cause.stack : cause
      )
    })),
    Effect.mapError((err: any) => {
      const cause = "cause" in err ? err.cause : err
      const msg = cause instanceof Error ? cause.message : (typeof cause === "string" ? cause : JSON.stringify(cause))
      return new StorageUnavailable({ message: `Storage service unavailable: ${msg}` })
    })
  )

const storageSummary = (config: StorageConfig) => ({
  endpoint: config.endpoint,
  region: config.region,
  bucket: config.bucket,
  accessKeyId: config.accessKeyId,
  forcePathStyle: config.forcePathStyle
})

const viewerUrl = (origin: string, id: string) =>
  `${origin}/v/${encodeURIComponent(id)}`

const playlistUrl = (origin: string, id: string) =>
  `${viewerUrl(origin, id)}/playlist.m3u8`

const decodeStorage = (encrypted: string) => Effect.gen(function*() {
  const vault = yield* Vault
  return yield* internal(vault.decrypt(encrypted))
})

const requestIsOwner = (video: VideoRecord) => Effect.gen(function*() {
  const auth = yield* Auth
  const request = yield* HttpServerRequest.HttpServerRequest
  const identity = yield* auth.verifySession(new Headers(request.headers))
  return identity?.id === video.userId
})

const requireOwnedVideo = (userId: string, id: string) => Effect.gen(function*() {
  const repository = yield* Repository
  const video = yield* internal(repository.findOwnedVideo(userId, id))
  return video ?? (yield* Effect.fail(new NotFound({ message: "Recording not found" })))
})

const setupAuthLayer = (setupToken: string) => Layer.succeed(SetupAuth)({
  bearer: (httpEffect, { credential }) => secureEqual(Redacted.value(credential), setupToken)
    ? httpEffect
    : Effect.fail(new Unauthorized({ message: "Invalid setup token" }))
})

const userAuthLayer = Layer.succeed(UserAuth)({
  bearer: (httpEffect, { credential }) => Effect.gen(function*() {
      const auth = yield* Auth
      const repository = yield* Repository
      const apiKey = Redacted.value(credential)
      const identity = apiKey.length > 0
        ? yield* auth.verifyApiKey(apiKey)
        : yield* Effect.gen(function*() {
            const request = yield* HttpServerRequest.HttpServerRequest
            return yield* auth.verifySession(new Headers(request.headers))
          })
      if (!identity) return yield* Effect.fail(new Unauthorized({ message: "Authentication required" }))
      const user = yield* repository.ensureUser(identity.id, identity.name, new Date().toISOString()).pipe(Effect.orDie)
      return yield* Effect.provideService(httpEffect, CurrentUser, {
        id: user.id,
        name: user.name,
        storageConfig: user.storageConfig
      })
    })
})

const makeSystemHandlers = (publicOrigin: string) => HttpApiBuilder.group(BlipApi, "system", (handlers) => handlers
  .handle("landing", () => Effect.succeed(landingPage))
  .handle("health", () => Effect.succeed({ ok: true }))
  .handle("view", ({ params }) => Effect.gen(function*() {
    const repository = yield* Repository
    const storage = yield* ObjectStorage
    const video = yield* internal(repository.findVideo(params.id))
    if (!video || video.status !== "complete") {
      return yield* Effect.fail(new NotFound({ message: "Recording not found" }))
    }
    const owner = yield* requestIsOwner(video)
    let source: string | undefined
    if (owner || video.privacy === "public") {
      const config = yield* decodeStorage(video.storageConfig)
      source = video.objectKey.endsWith("/playlist.m3u8")
        ? playlistUrl(publicOrigin, video.id)
        : yield* unavailable(storage.signView(config, video.objectKey))
    } else if (video.privacy === "password") {
      if (video.passwordHash && (yield* getVerifiedPasswordHashes).includes(video.passwordHash)) {
        const config = yield* decodeStorage(video.storageConfig)
        source = video.objectKey.endsWith("/playlist.m3u8")
          ? playlistUrl(publicOrigin, video.id)
          : yield* unavailable(storage.signView(config, video.objectKey))
      } else {
        return yield* Effect.fail(new PasswordRequired({ message: "This recording requires a password" }))
      }
    }
    return {
      id: video.id,
      ...(owner ? { name: video.name, createdAt: video.createdAt } : {}),
      privacy: video.privacy,
      owner,
      ...(source ? { source } : {})
    }
  }))
  .handle("playlist", ({ params }) => Effect.gen(function*() {
    const repository = yield* Repository
    const storage = yield* ObjectStorage
    const video = yield* internal(repository.findVideo(params.id))
    if (!video || video.status !== "complete" || !video.objectKey.endsWith("/playlist.m3u8")) {
      return yield* Effect.fail(new NotFound({ message: "Recording not found" }))
    }
    const owner = yield* requestIsOwner(video)
    if (!owner && video.privacy !== "public") {
      if (video.privacy === "password" && video.passwordHash && (yield* getVerifiedPasswordHashes).includes(video.passwordHash)) {
        // Authorized via cookie
      } else if (video.privacy === "password") {
        return yield* Effect.fail(new PasswordRequired({ message: "This recording requires a password" }))
      } else {
        return yield* Effect.fail(new NotFound({ message: "Recording not found" }))
      }
    }
    const config = yield* decodeStorage(video.storageConfig)
    const playlist = yield* unavailable(storage.readText(config, video.objectKey))
    const prefix = video.objectKey.slice(0, video.objectKey.lastIndexOf("/") + 1)
    const names = [...new Set(playlist.match(/(?:init\.mp4|segment\d{5}\.m4s)/g) ?? [])]
    const urls = yield* Effect.all(names.map((name) => unavailable(storage.signView(config, `${prefix}${name}`))))
    return names.reduce((value, name, index) => value.replaceAll(name, urls[index]!), playlist)
  }))
  .handle("unlock", ({ params, payload }) => Effect.gen(function*() {
    const repository = yield* Repository
    const video = yield* internal(repository.findVideo(params.id))
    if (!video || video.status !== "complete" || video.privacy !== "password" || !video.passwordHash) {
      return yield* Effect.fail(new NotFound({ message: "Recording not found" }))
    }
    if (!(yield* verifyPassword(payload.password, video.passwordHash))) {
      return yield* Effect.fail(new Unauthorized({ message: "Incorrect password" }))
    }
    yield* setVerifiedPasswordCookie(video.passwordHash)
    return { ok: true }
  })))

const userHandlers = HttpApiBuilder.group(BlipApi, "users", (handlers) => handlers
  .handle("create", ({ payload }) => Effect.gen(function*() {
    const repository = yield* Repository
    const apiKey = randomToken("blip_")
    const id = crypto.randomUUID()
    yield* internal(repository.createUser({
      id,
      name: payload.name,
      apiKeyHash: yield* hashApiKey(apiKey),
      storageConfig: null
    }, new Date().toISOString()))
    return { id, name: payload.name, apiKey }
  })))

const storageHandlers = HttpApiBuilder.group(BlipApi, "storage", (handlers) => handlers
  .handle("get", () => Effect.gen(function*() {
    const user = yield* CurrentUser
    if (!user.storageConfig) return yield* Effect.fail(new NotFound({ message: "Storage is not configured" }))
    return storageSummary(yield* decodeStorage(user.storageConfig))
  }))
  .handle("set", ({ payload }) => Effect.gen(function*() {
    const user = yield* CurrentUser
    const repository = yield* Repository
    const storage = yield* ObjectStorage
    const vault = yield* Vault
    let endpoint: URL
    try {
      endpoint = new URL(payload.endpoint)
    } catch {
      return yield* Effect.fail(new InvalidRequest({ message: "Storage endpoint must be a valid URL" }))
    }
    if (endpoint.protocol !== "https:" && endpoint.hostname !== "localhost") {
      return yield* Effect.fail(new InvalidRequest({ message: "Storage endpoint must use HTTPS" }))
    }
    yield* unavailable(storage.test(payload))
    yield* internal(repository.setStorageConfig(user.id, yield* internal(vault.encrypt(payload))))
    return storageSummary(payload)
  }))
  .handle("remove", () => Effect.gen(function*() {
    const user = yield* CurrentUser
    const repository = yield* Repository
    yield* internal(repository.setStorageConfig(user.id, null))
  })))

const makeUploadHandlers = (publicOrigin: string) => HttpApiBuilder.group(BlipApi, "uploads", (handlers) => handlers
  .handle("list", () => Effect.gen(function*() {
    const user = yield* CurrentUser
    const repository = yield* Repository
    const videos = yield* internal(repository.listOwnedVideos(user.id))
    return videos.map((video) => ({
      id: video.id,
      name: video.name,
      status: video.status,
      format: video.objectKey.endsWith("/playlist.m3u8") ? "hls" : "mp4",
      createdAt: video.createdAt,
      viewerUrl: viewerUrl(publicOrigin, video.id),
      privacy: video.privacy
    }))
  }))
  .handle("create", ({ payload }) => Effect.gen(function*() {
    const user = yield* CurrentUser
    const repository = yield* Repository
    const storage = yield* ObjectStorage
    if (!user.storageConfig) return yield* Effect.fail(new NotFound({ message: "Storage is not configured" }))
    if (payload.size !== undefined && (payload.size <= 0 || payload.size > MAX_RECORDING_SIZE)) {
      return yield* Effect.fail(new InvalidRequest({ message: "Recording size is outside the supported range" }))
    }
    const config = yield* decodeStorage(user.storageConfig)
    const id = crypto.randomUUID()
    if (payload.format !== undefined && payload.format !== "mp4" && payload.format !== "hls") {
      return yield* Effect.fail(new InvalidRequest({ message: "Recording format must be MP4 or HLS" }))
    }
    const extensionIndex = payload.filename.lastIndexOf(".")
    const name = (extensionIndex > 0 ? payload.filename.slice(0, extensionIndex) : payload.filename).trim()
    if (name.length === 0) {
      return yield* Effect.fail(new InvalidRequest({ message: "Recording name is required" }))
    }
    const hls = payload.format === "hls"
    const objectKey = hls
      ? `users/${user.id}/recordings/${id}/playlist.m3u8`
      : `users/${user.id}/recordings/${id}.mp4`
    const uploadId = yield* unavailable(storage.createUpload(
      config,
      objectKey,
      hls ? "application/vnd.apple.mpegurl" : "video/mp4"
    ))
    const save = internal(repository.createVideo({
      id,
      userId: user.id,
      name,
      objectKey,
      uploadId,
      storageConfig: user.storageConfig,
      status: "uploading",
      privacy: "public",
      passwordHash: null
    }, new Date().toISOString()))
    yield* save.pipe(Effect.catch((error) => storage.abortUpload(config, objectKey, uploadId).pipe(
      Effect.ignore,
      Effect.andThen(Effect.fail(error))
    )))
    return {
      id,
      uploadId,
      partSize: PART_SIZE,
      viewerUrl: viewerUrl(publicOrigin, id)
    }
  }))
  .handle("signPart", ({ params, payload }) => Effect.gen(function*() {
    const user = yield* CurrentUser
    const storage = yield* ObjectStorage
    const video = yield* requireOwnedVideo(user.id, params.id)
    if (video.uploadId !== payload.uploadId || video.status !== "uploading") {
      return yield* Effect.fail(new InvalidRequest({ message: "Invalid upload session" }))
    }
    const config = yield* decodeStorage(video.storageConfig)
    return { url: yield* unavailable(storage.signPart(config, video.objectKey, video.uploadId, payload.partNumber)) }
  }))
  .handle("signAsset", ({ params, payload }) => Effect.gen(function*() {
    const user = yield* CurrentUser
    const storage = yield* ObjectStorage
    const video = yield* requireOwnedVideo(user.id, params.id)
    if (video.uploadId !== payload.uploadId || video.status !== "uploading" || !video.objectKey.endsWith("/playlist.m3u8")) {
      return yield* Effect.fail(new InvalidRequest({ message: "Invalid HLS upload session" }))
    }
    const contentType = payload.name === "init.mp4"
      ? "video/mp4"
      : /^segment\d{5}\.m4s$/.test(payload.name)
        ? "video/iso.segment"
        : undefined
    if (!contentType) {
      return yield* Effect.fail(new InvalidRequest({ message: "Invalid HLS asset name" }))
    }
    const config = yield* decodeStorage(video.storageConfig)
    const prefix = video.objectKey.slice(0, video.objectKey.lastIndexOf("/") + 1)
    return {
      url: yield* unavailable(storage.signPut(config, `${prefix}${payload.name}`, contentType)),
      contentType
    }
  }))
  .handle("complete", ({ params, payload }) => Effect.gen(function*() {
    const user = yield* CurrentUser
    const repository = yield* Repository
    const storage = yield* ObjectStorage
    const video = yield* requireOwnedVideo(user.id, params.id)
    if (video.uploadId !== payload.uploadId) {
      return yield* Effect.fail(new InvalidRequest({ message: "Invalid upload session" }))
    }
    if (video.status === "complete") {
      return { viewerUrl: viewerUrl(publicOrigin, video.id) }
    }
    const numbers = new Set(payload.parts.map((part) => part.partNumber))
    if (payload.parts.length === 0 || numbers.size !== payload.parts.length) {
      return yield* Effect.fail(new InvalidRequest({ message: "Parts must be non-empty and unique" }))
    }
    const config = yield* decodeStorage(video.storageConfig)
    const completeOrReconcile = storage.completeUpload(
      config,
      video.objectKey,
      video.uploadId,
      payload.parts
    ).pipe(Effect.catch(() => storage.objectExists(config, video.objectKey).pipe(
      Effect.flatMap((exists) => exists
        ? Effect.void
        : Effect.fail(new StorageUnavailable({ message: "Storage service unavailable" })))
    )))
    yield* unavailable(completeOrReconcile)
    yield* internal(repository.markVideoComplete(video.id))
    return { viewerUrl: viewerUrl(publicOrigin, video.id) }
  }))
  .handle("abort", ({ params, payload }) => Effect.gen(function*() {
    const user = yield* CurrentUser
    const repository = yield* Repository
    const storage = yield* ObjectStorage
    const video = yield* requireOwnedVideo(user.id, params.id)
    if (video.uploadId !== payload.uploadId || video.status !== "uploading") {
      return yield* Effect.fail(new InvalidRequest({ message: "Invalid upload session" }))
    }
    const config = yield* decodeStorage(video.storageConfig)
    yield* unavailable(storage.abortUpload(config, video.objectKey, video.uploadId))
    yield* internal(repository.deleteVideo(video.id))
  }))
  .handle("rename", ({ params, payload }) => Effect.gen(function*() {
    const user = yield* CurrentUser
    const repository = yield* Repository
    const video = yield* requireOwnedVideo(user.id, params.id)
    const name = payload.name.trim()
    if (name.length === 0 || name.length > 256) {
      return yield* Effect.fail(new InvalidRequest({ message: "Recording name must be between 1 and 256 characters" }))
    }
    yield* internal(repository.setVideoName(video.id, name))
    return { name }
  }))
  .handle("privacy", ({ params, payload }) => Effect.gen(function*() {
    const user = yield* CurrentUser
    const repository = yield* Repository
    const video = yield* requireOwnedVideo(user.id, params.id)
    if (payload.privacy !== "public" && payload.privacy !== "password" && payload.privacy !== "private") {
      return yield* Effect.fail(new InvalidRequest({ message: "Invalid recording privacy setting" }))
    }
    let passwordHash: string | null = null
    if (payload.privacy === "password") {
      if (payload.password !== undefined && (payload.password.length < 4 || payload.password.length > 256)) {
        return yield* Effect.fail(new InvalidRequest({ message: "Password must be between 4 and 256 characters" }))
      }
      passwordHash = payload.password === undefined ? video.passwordHash : yield* hashPassword(payload.password)
      if (!passwordHash) {
        return yield* Effect.fail(new InvalidRequest({ message: "Enter a password for this recording" }))
      }
    }
    yield* internal(repository.setVideoPrivacy(video.id, payload.privacy, passwordHash))
    return { privacy: payload.privacy }
  }))
  .handle("archive", ({ params }) => Effect.gen(function*() {
    const user = yield* CurrentUser
    const repository = yield* Repository
    const video = yield* requireOwnedVideo(user.id, params.id)
    yield* internal(repository.archiveVideo(video.id, new Date().toISOString()))
  })))

export const layer = (setupToken: string, publicOrigin: string) =>
  Layer.mergeAll(
    HttpApiBuilder.layer(BlipApi, { openapiPath: "/openapi.json" }),
    HttpApiScalar.layer(BlipApi, { path: "/api" })
  ).pipe(
    Layer.provide([makeSystemHandlers(publicOrigin), userHandlers, storageHandlers, makeUploadHandlers(publicOrigin)]),
    Layer.provide([setupAuthLayer(setupToken), userAuthLayer])
  )

export const provideServices = HttpRouter.provideRequest

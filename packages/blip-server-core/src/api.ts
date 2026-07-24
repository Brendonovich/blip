import * as Context from "effect/Context"
import * as Schema from "effect/Schema"
import * as HttpApi from "effect/unstable/httpapi/HttpApi"
import * as HttpApiEndpoint from "effect/unstable/httpapi/HttpApiEndpoint"
import * as HttpApiGroup from "effect/unstable/httpapi/HttpApiGroup"
import * as HttpApiMiddleware from "effect/unstable/httpapi/HttpApiMiddleware"
import * as HttpApiSchema from "effect/unstable/httpapi/HttpApiSchema"
import * as HttpApiSecurity from "effect/unstable/httpapi/HttpApiSecurity"
import { StorageConfig } from "./domain/model.ts"

export class Unauthorized extends Schema.TaggedErrorClass<Unauthorized>()(
  "Unauthorized",
  { message: Schema.String },
  { httpApiStatus: 401 }
) {}

export class NotFound extends Schema.TaggedErrorClass<NotFound>()(
  "NotFound",
  { message: Schema.String },
  { httpApiStatus: 404 }
) {}

export class InvalidRequest extends Schema.TaggedErrorClass<InvalidRequest>()(
  "InvalidRequest",
  { message: Schema.String },
  { httpApiStatus: 400 }
) {}

export class StorageUnavailable extends Schema.TaggedErrorClass<StorageUnavailable>()(
  "StorageUnavailable",
  { message: Schema.String },
  { httpApiStatus: 502 }
) {}

export class PasswordRequired extends Schema.TaggedErrorClass<PasswordRequired>()(
  "PasswordRequired",
  { message: Schema.String },
  { httpApiStatus: 403 }
) {}

export class InternalError extends Schema.TaggedErrorClass<InternalError>()(
  "InternalError",
  { message: Schema.String },
  { httpApiStatus: 500 }
) {}

export class CurrentUser extends Context.Service<CurrentUser, {
  readonly id: string
  readonly name: string
  readonly storageConfig: string | null
}>()("blip/CurrentUser") {}

export class VerifiedPasswordHashes extends Context.Service<VerifiedPasswordHashes, {
  readonly hashes: ReadonlyArray<string>
}>()("blip/VerifiedPasswordHashes") {}

export class UserAuth extends HttpApiMiddleware.Service<UserAuth, {
  provides: CurrentUser
  requires: import("./domain/auth.ts").Auth | import("./domain/repository.ts").Repository
}>()("blip/UserAuth", {
  error: Unauthorized,
  security: { bearer: HttpApiSecurity.bearer }
}) {}

export class SetupAuth extends HttpApiMiddleware.Service<SetupAuth>()(
  "blip/SetupAuth",
  {
    error: Unauthorized,
    security: { bearer: HttpApiSecurity.bearer }
  }
) {}

const IdParams = { id: Schema.NonEmptyString }
const PrivacyPayload = Schema.Struct({
  privacy: Schema.String,
  password: Schema.optional(Schema.String)
})
const PartNumber = Schema.Int.check(Schema.isBetween({ minimum: 1, maximum: 10_000 }))
const ApiErrors = [InvalidRequest, StorageUnavailable, InternalError] as const

const Health = Schema.Struct({ ok: Schema.Boolean })
const Html = Schema.String.pipe(HttpApiSchema.asText({ contentType: "text/html; charset=utf-8" }))
const HlsPlaylist = Schema.String.pipe(HttpApiSchema.asText({ contentType: "application/vnd.apple.mpegurl" }))
const VideoView = Schema.Struct({
  id: Schema.String,
  privacy: Schema.String,
  owner: Schema.Boolean,
  source: Schema.optional(Schema.String)
})
const UserCreated = Schema.Struct({
  id: Schema.String,
  name: Schema.String,
  apiKey: Schema.String
}).pipe(HttpApiSchema.status("Created"))
const StorageSummary = Schema.Struct({
  endpoint: Schema.String,
  region: Schema.String,
  bucket: Schema.String,
  accessKeyId: Schema.String,
  forcePathStyle: Schema.Boolean
})
const UploadCreated = Schema.Struct({
  id: Schema.String,
  uploadId: Schema.String,
  partSize: Schema.Int,
  viewerUrl: Schema.String
}).pipe(HttpApiSchema.status("Created"))
const SignedPart = Schema.Struct({ url: Schema.String })
const CompletedUpload = Schema.Struct({ viewerUrl: Schema.String })
const RecordingSummary = Schema.Struct({
  id: Schema.String,
  status: Schema.String,
  format: Schema.String,
  createdAt: Schema.String,
  viewerUrl: Schema.String,
  privacy: Schema.String
})

const system = HttpApiGroup.make("system")
  .add(HttpApiEndpoint.get("landing", "/", { success: Html }))
  .add(HttpApiEndpoint.get("health", "/health", { success: Health }))
  .add(HttpApiEndpoint.get("view", "/api/view/:id", {
    params: IdParams,
    success: VideoView,
    error: [PasswordRequired, NotFound, StorageUnavailable, InternalError]
  }))
  .add(HttpApiEndpoint.get("playlist", "/v/:id/playlist.m3u8", {
    params: IdParams,
    success: HlsPlaylist,
    error: [PasswordRequired, NotFound, StorageUnavailable, InternalError]
  }))
  .add(HttpApiEndpoint.post("unlock", "/v/:id/unlock", {
    params: IdParams,
    payload: Schema.Struct({ password: Schema.String }),
    success: Schema.Struct({ ok: Schema.Boolean }),
    error: [Unauthorized, NotFound, StorageUnavailable, InternalError]
  }))

const users = HttpApiGroup.make("users")
  .add(HttpApiEndpoint.post("create", "/api/users", {
    payload: Schema.Struct({ name: Schema.NonEmptyString }),
    success: UserCreated,
    error: InternalError
  }).middleware(SetupAuth))

const storage = HttpApiGroup.make("storage")
  .add(HttpApiEndpoint.get("get", "/api/storage", {
    success: StorageSummary,
    error: [NotFound, InternalError]
  }).middleware(UserAuth))
  .add(HttpApiEndpoint.put("set", "/api/storage", {
    payload: StorageConfig,
    success: StorageSummary,
    error: ApiErrors
  }).middleware(UserAuth))
  .add(HttpApiEndpoint.delete("remove", "/api/storage", {
    success: HttpApiSchema.NoContent,
    error: InternalError
  }).middleware(UserAuth))

const uploads = HttpApiGroup.make("uploads")
  .add(HttpApiEndpoint.get("list", "/api/uploads", {
    success: Schema.Array(RecordingSummary),
    error: InternalError
  }).middleware(UserAuth))
  .add(HttpApiEndpoint.post("create", "/api/uploads", {
    payload: Schema.Struct({
      filename: Schema.optional(Schema.String),
      size: Schema.optional(Schema.Int),
      format: Schema.optional(Schema.String)
    }),
    success: UploadCreated,
    error: [NotFound, ...ApiErrors]
  }).middleware(UserAuth))
  .add(HttpApiEndpoint.post("signPart", "/api/uploads/:id/parts", {
    params: IdParams,
    payload: Schema.Struct({
      uploadId: Schema.NonEmptyString,
      partNumber: PartNumber
    }),
    success: SignedPart,
    error: [NotFound, ...ApiErrors]
  }).middleware(UserAuth))
  .add(HttpApiEndpoint.post("signAsset", "/api/uploads/:id/assets", {
    params: IdParams,
    payload: Schema.Struct({
      uploadId: Schema.NonEmptyString,
      name: Schema.NonEmptyString
    }),
    success: Schema.Struct({ url: Schema.String, contentType: Schema.String }),
    error: [NotFound, ...ApiErrors]
  }).middleware(UserAuth))
  .add(HttpApiEndpoint.post("complete", "/api/uploads/:id/complete", {
    params: IdParams,
    payload: Schema.Struct({
      uploadId: Schema.NonEmptyString,
      parts: Schema.Array(Schema.Struct({
        partNumber: PartNumber,
        etag: Schema.NonEmptyString
      }))
    }),
    success: CompletedUpload,
    error: [NotFound, ...ApiErrors]
  }).middleware(UserAuth))
  .add(HttpApiEndpoint.delete("abort", "/api/uploads/:id", {
    params: IdParams,
    payload: Schema.Struct({ uploadId: Schema.NonEmptyString }),
    success: HttpApiSchema.NoContent,
    error: [NotFound, ...ApiErrors]
  }).middleware(UserAuth))
  .add(HttpApiEndpoint.put("privacy", "/api/uploads/:id/privacy", {
    params: IdParams,
    payload: PrivacyPayload,
    success: Schema.Struct({ privacy: Schema.String }),
    error: [NotFound, ...ApiErrors]
  }).middleware(UserAuth))

export class BlipApi extends HttpApi.make("BlipApi")
  .add(system)
  .add(users)
  .add(storage)
  .add(uploads) {}

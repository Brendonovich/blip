import * as AWS from "@distilled.cloud/aws"
import * as S3 from "@distilled.cloud/aws/s3"
import * as Context from "effect/Context"
import * as Effect from "effect/Effect"
import * as Layer from "effect/Layer"
import * as Stream from "effect/Stream"
import * as FetchHttpClient from "effect/unstable/http/FetchHttpClient"
import * as HttpClient from "effect/unstable/http/HttpClient"
import { ObjectStorageError } from "./errors.ts"
import type { StorageConfig } from "./model.ts"

const UPLOAD_URL_TTL_SECONDS = 15 * 60
const VIEW_URL_TTL_SECONDS = 60 * 60

export interface ObjectStorageService {
  readonly test: (config: StorageConfig) => Effect.Effect<void, ObjectStorageError>
  readonly createUpload: (config: StorageConfig, key: string, contentType: string) => Effect.Effect<string, ObjectStorageError>
  readonly signPart: (config: StorageConfig, key: string, uploadId: string, partNumber: number) => Effect.Effect<string, ObjectStorageError>
  readonly completeUpload: (config: StorageConfig, key: string, uploadId: string, parts: ReadonlyArray<{ readonly partNumber: number; readonly etag: string }>) => Effect.Effect<void, ObjectStorageError>
  readonly abortUpload: (config: StorageConfig, key: string, uploadId: string) => Effect.Effect<void, ObjectStorageError>
  readonly objectExists: (config: StorageConfig, key: string) => Effect.Effect<boolean, ObjectStorageError>
  readonly signView: (config: StorageConfig, key: string) => Effect.Effect<string, ObjectStorageError>
  readonly signPut: (config: StorageConfig, key: string, contentType: string) => Effect.Effect<string, ObjectStorageError>
  readonly readText: (config: StorageConfig, key: string) => Effect.Effect<string, ObjectStorageError>
}

export class ObjectStorage extends Context.Service<ObjectStorage, ObjectStorageService>()(
  "blip/ObjectStorage"
) {}

const run = <A, E>(
  config: StorageConfig,
  effect: Effect.Effect<
    A,
    E,
    AWS.Credentials.Credentials | AWS.Region.Region | HttpClient.HttpClient
  >
) => effect.pipe(
  Effect.provide(AWS.Credentials.fromCredentials({
    accessKeyId: config.accessKeyId,
    secretAccessKey: config.secretAccessKey
  })),
  Effect.provideService(AWS.Region.Region, Effect.succeed(config.region)),
  Effect.provideService(AWS.Endpoint.Endpoint, Effect.succeed(config.endpoint)),
  Effect.provide(FetchHttpClient.layer),
  Effect.mapError((cause) => new ObjectStorageError({ cause }))
)

const objectUrl = (config: StorageConfig, key: string) => {
  const url = new URL(config.endpoint)
  const path = [config.bucket, ...key.split("/")]
    .map(encodeURIComponent)
    .join("/")
  url.pathname = `${url.pathname.replace(/\/$/, "")}/${path}`
  return url
}

const presign = (
  config: StorageConfig,
  key: string,
  method: "GET" | "PUT",
  contentType?: string,
  query?: Record<string, string>
) => {
  const url = objectUrl(config, key)
  for (const [name, value] of Object.entries(query ?? {})) {
    url.searchParams.set(name, value)
  }
  return run(config, AWS.Presign.presignUrl({
    method,
    url: url.toString(),
    service: "s3",
    region: config.region,
    expiresIn: method === "GET" ? VIEW_URL_TTL_SECONDS : UPLOAD_URL_TTL_SECONDS,
    ...(contentType ? { headers: { "content-type": contentType } } : {})
  }))
}

const normalizeEtag = (etag: string) => {
  const value = etag.trim()
  return value.startsWith('"') && value.endsWith('"') ? value : `"${value}"`
}

export const layer = Layer.succeed(ObjectStorage)({
  test: (config) => run(config, Effect.gen(function*() {
    const key = `.blip-connection-test-${crypto.randomUUID()}`
    const result = yield* S3.createMultipartUpload({
      Bucket: config.bucket,
      Key: key
    })
    if (!result.UploadId) throw new Error("Storage did not return an upload ID")
    yield* S3.abortMultipartUpload({
      Bucket: config.bucket,
      Key: key,
      UploadId: result.UploadId
    })
  })),
  createUpload: (config, key, contentType) => run(config, Effect.gen(function*() {
    const result = yield* S3.createMultipartUpload({
      Bucket: config.bucket,
      Key: key,
      ContentType: contentType,
      CacheControl: "private, max-age=0"
    })
    if (!result.UploadId) throw new Error("Storage did not return an upload ID")
    return result.UploadId
  })),
  signPart: (config, key, uploadId, partNumber) => presign(
    config,
    key,
    "PUT",
    undefined,
    { uploadId, partNumber: String(partNumber) }
  ),
  completeUpload: (config, key, uploadId, parts) => run(config,
    S3.completeMultipartUpload({
      Bucket: config.bucket,
      Key: key,
      UploadId: uploadId,
      MultipartUpload: {
        Parts: [...parts]
          .sort((left, right) => left.partNumber - right.partNumber)
          .map((part) => ({ PartNumber: part.partNumber, ETag: normalizeEtag(part.etag) }))
      }
    }).pipe(Effect.asVoid)
  ),
  abortUpload: (config, key, uploadId) => run(config,
    S3.abortMultipartUpload({ Bucket: config.bucket, Key: key, UploadId: uploadId }).pipe(Effect.asVoid)
  ),
  objectExists: (config, key) => run(config,
    S3.headObject({ Bucket: config.bucket, Key: key }).pipe(
      Effect.as(true),
      Effect.catchTag("NotFound", () => Effect.succeed(false))
    )
  ),
  signView: (config, key) => presign(config, key, "GET"),
  signPut: (config, key, contentType) => presign(config, key, "PUT", contentType),
  readText: (config, key) => run(config, Effect.gen(function*() {
    const result = yield* S3.getObject({ Bucket: config.bucket, Key: key })
    if (!result.Body) throw new Error("Storage returned an empty object")
    return yield* result.Body.pipe(
      Stream.decodeText(),
      Stream.runFold(() => "", (text, chunk) => text + chunk)
    )
  }))
})

import * as Schema from "effect/Schema"

export const StorageConfig = Schema.Struct({
  endpoint: Schema.NonEmptyString,
  region: Schema.NonEmptyString,
  bucket: Schema.NonEmptyString,
  accessKeyId: Schema.NonEmptyString,
  secretAccessKey: Schema.NonEmptyString,
  forcePathStyle: Schema.Boolean
})

export type StorageConfig = Schema.Schema.Type<typeof StorageConfig>
export type VideoPrivacy = "public" | "password" | "private"

export interface UserRecord {
  readonly id: string
  readonly name: string
  readonly apiKeyHash: string
  readonly storageConfig: string | null
}

export interface VideoRecord {
  readonly id: string
  readonly userId: string
  readonly name: string
  readonly createdAt: string
  readonly objectKey: string
  readonly uploadId: string
  readonly storageConfig: string
  readonly status: "uploading" | "complete"
  readonly privacy: VideoPrivacy
  readonly passwordHash: string | null
}

import * as Context from "effect/Context"
import * as Effect from "effect/Effect"
import * as Layer from "effect/Layer"
import * as SqlClient from "effect/unstable/sql/SqlClient"
import type { RepositoryError } from "./errors.ts"
import type { UserRecord, VideoPrivacy, VideoRecord } from "./model.ts"
import { RepositoryError as RepositoryFailure } from "./errors.ts"

export interface RepositoryService {
  readonly createUser: (user: UserRecord, createdAt: string) => Effect.Effect<void, RepositoryError>
  readonly ensureUser: (id: string, name: string, createdAt: string) => Effect.Effect<UserRecord, RepositoryError>
  readonly findUserByApiKeyHash: (apiKey: string) => Effect.Effect<UserRecord | undefined, RepositoryError>
  readonly setStorageConfig: (userId: string, encrypted: string | null) => Effect.Effect<void, RepositoryError>
  readonly createVideo: (video: VideoRecord, createdAt: string) => Effect.Effect<void, RepositoryError>
  readonly findOwnedVideo: (userId: string, id: string) => Effect.Effect<VideoRecord | undefined, RepositoryError>
  readonly findVideo: (id: string) => Effect.Effect<VideoRecord | undefined, RepositoryError>
  readonly listOwnedVideos: (userId: string) => Effect.Effect<ReadonlyArray<VideoSummaryRecord>, RepositoryError>
  readonly setVideoPrivacy: (id: string, privacy: VideoPrivacy, passwordHash: string | null) => Effect.Effect<void, RepositoryError>
  readonly markVideoComplete: (id: string) => Effect.Effect<void, RepositoryError>
  readonly deleteVideo: (id: string) => Effect.Effect<void, RepositoryError>
}

export interface VideoSummaryRecord {
  readonly id: string
  readonly objectKey: string
  readonly status: "uploading" | "complete"
  readonly createdAt: string
  readonly privacy: VideoPrivacy
}

export class Repository extends Context.Service<Repository, RepositoryService>()(
  "blip/Repository"
) {}

interface UserRow {
  readonly id: string
  readonly name: string
  readonly api_key_hash: string
  readonly storage_config: string | null
}

interface VideoRow {
  readonly id: string
  readonly user_id: string
  readonly object_key: string
  readonly upload_id: string
  readonly storage_config: string
  readonly status: "uploading" | "complete"
  readonly privacy: VideoPrivacy
  readonly password_hash: string | null
}

interface VideoSummaryRow {
  readonly id: string
  readonly object_key: string
  readonly status: "uploading" | "complete"
  readonly created_at: string
  readonly privacy: VideoPrivacy
}

const userFromRow = (row: UserRow): UserRecord => ({
  id: row.id,
  name: row.name,
  apiKeyHash: row.api_key_hash,
  storageConfig: row.storage_config
})

const videoFromRow = (row: VideoRow): VideoRecord => ({
  id: row.id,
  userId: row.user_id,
  objectKey: row.object_key,
  uploadId: row.upload_id,
  storageConfig: row.storage_config,
  status: row.status,
  privacy: row.privacy,
  passwordHash: row.password_hash
})

const databaseError = <A, R>(effect: Effect.Effect<A, unknown, R>) =>
  Effect.catchCause(effect, (cause) => Effect.fail(new RepositoryFailure({ cause })))

export const layer = Layer.effect(Repository)(
  Effect.gen(function*() {
    const sql = yield* SqlClient.SqlClient
    return Repository.of({
      createUser: (user, createdAt) => databaseError(
        sql`INSERT INTO users (id, name, api_key_hash, storage_config, created_at)
            VALUES (${user.id}, ${user.name}, ${user.apiKeyHash}, ${user.storageConfig}, ${createdAt})`
      ).pipe(Effect.asVoid),
      ensureUser: (id, name, createdAt) => databaseError(Effect.gen(function*() {
        yield* sql`INSERT INTO users (id, name, api_key_hash, storage_config, created_at)
                   VALUES (${id}, ${name}, ${`better_auth:${id}`}, NULL, ${createdAt})
                   ON CONFLICT(id) DO UPDATE SET name = excluded.name`
        const rows = yield* sql<UserRow>`SELECT id, name, api_key_hash, storage_config
                                         FROM users WHERE id = ${id} LIMIT 1`
        return userFromRow(rows[0]!)
      })),
      findUserByApiKeyHash: (apiKey) => databaseError(Effect.promise(async () => {
        const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(apiKey))
        return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, "0")).join("")
      })).pipe(
        Effect.flatMap((hash) => databaseError(
          sql<UserRow>`SELECT id, name, api_key_hash, storage_config
                       FROM users WHERE api_key_hash = ${hash} LIMIT 1`
        )),
        Effect.map((rows) => rows[0] ? userFromRow(rows[0]) : undefined)
      ),
      setStorageConfig: (userId, encrypted) => databaseError(
        sql`UPDATE users SET storage_config = ${encrypted} WHERE id = ${userId}`
      ).pipe(Effect.asVoid),
      createVideo: (video, createdAt) => databaseError(
        sql`INSERT INTO videos
              (id, user_id, object_key, upload_id, storage_config, status, privacy, password_hash, created_at)
            VALUES
              (${video.id}, ${video.userId}, ${video.objectKey}, ${video.uploadId},
               ${video.storageConfig}, ${video.status}, ${video.privacy},
               ${video.passwordHash}, ${createdAt})`
      ).pipe(Effect.asVoid),
      findOwnedVideo: (userId, id) => databaseError(
        sql<VideoRow>`SELECT id, user_id, object_key, upload_id, storage_config, status, privacy, password_hash
                      FROM videos WHERE id = ${id} AND user_id = ${userId} LIMIT 1`
      ).pipe(Effect.map((rows) => rows[0] ? videoFromRow(rows[0]) : undefined)),
      findVideo: (id) => databaseError(
        sql<VideoRow>`SELECT id, user_id, object_key, upload_id, storage_config, status, privacy, password_hash
                      FROM videos WHERE id = ${id} LIMIT 1`
      ).pipe(Effect.map((rows) => rows[0] ? videoFromRow(rows[0]) : undefined)),
      listOwnedVideos: (userId) => databaseError(
        sql<VideoSummaryRow>`SELECT id, object_key, status, created_at, privacy
                             FROM videos WHERE user_id = ${userId}
                             ORDER BY created_at DESC`
      ).pipe(Effect.map((rows) => rows.map((row) => ({
        id: row.id,
        objectKey: row.object_key,
        status: row.status,
        createdAt: row.created_at,
        privacy: row.privacy
      })))),
      setVideoPrivacy: (id, privacy, passwordHash) => databaseError(
        sql`UPDATE videos SET privacy = ${privacy}, password_hash = ${passwordHash} WHERE id = ${id}`
      ).pipe(Effect.asVoid),
      markVideoComplete: (id) => databaseError(
        sql`UPDATE videos SET status = 'complete' WHERE id = ${id}`
      ).pipe(Effect.asVoid),
      deleteVideo: (id) => databaseError(
        sql`DELETE FROM videos WHERE id = ${id}`
      ).pipe(Effect.asVoid)
    })
  })
)

export const initialize = Effect.gen(function*() {
  const sql = yield* SqlClient.SqlClient
  yield* sql`CREATE TABLE IF NOT EXISTS users (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    api_key_hash TEXT NOT NULL UNIQUE,
    storage_config TEXT,
    created_at TEXT NOT NULL
  )`
  yield* sql`CREATE TABLE IF NOT EXISTS videos (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    object_key TEXT NOT NULL,
    upload_id TEXT NOT NULL,
    storage_config TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('uploading', 'complete')),
    privacy TEXT NOT NULL DEFAULT 'public' CHECK (privacy IN ('public', 'password', 'private')),
    password_hash TEXT,
    created_at TEXT NOT NULL
  )`
  yield* sql`CREATE INDEX IF NOT EXISTS videos_user_id ON videos(user_id)`
}).pipe(Effect.asVoid)

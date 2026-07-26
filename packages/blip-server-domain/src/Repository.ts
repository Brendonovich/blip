import * as Context from "effect/Context"
import type * as Effect from "effect/Effect"
import type { RepositoryError } from "./Errors.ts"
import type { UserRecord, VideoPrivacy, VideoRecord } from "./Model.ts"

export interface Interface {
  readonly createUser: (user: UserRecord, createdAt: string) => Effect.Effect<void, RepositoryError>
  readonly ensureUser: (id: string, name: string, createdAt: string) => Effect.Effect<UserRecord, RepositoryError>
  readonly findUserByApiKeyHash: (apiKey: string) => Effect.Effect<UserRecord | undefined, RepositoryError>
  readonly setStorageConfig: (userId: string, encrypted: string | null) => Effect.Effect<void, RepositoryError>
  readonly createVideo: (video: Omit<VideoRecord, "createdAt">, createdAt: string) => Effect.Effect<void, RepositoryError>
  readonly findOwnedVideo: (userId: string, id: string) => Effect.Effect<VideoRecord | undefined, RepositoryError>
  readonly findVideo: (id: string) => Effect.Effect<VideoRecord | undefined, RepositoryError>
  readonly listOwnedVideos: (userId: string) => Effect.Effect<ReadonlyArray<VideoSummaryRecord>, RepositoryError>
  readonly setVideoName: (id: string, name: string) => Effect.Effect<void, RepositoryError>
  readonly setVideoPrivacy: (id: string, privacy: VideoPrivacy, passwordHash: string | null) => Effect.Effect<void, RepositoryError>
  readonly archiveVideo: (id: string, archivedAt: string) => Effect.Effect<void, RepositoryError>
  readonly markVideoComplete: (id: string) => Effect.Effect<void, RepositoryError>
  readonly deleteVideo: (id: string) => Effect.Effect<void, RepositoryError>
}

export interface VideoSummaryRecord {
  readonly id: string
  readonly name: string
  readonly objectKey: string
  readonly status: "uploading" | "complete"
  readonly createdAt: string
  readonly privacy: VideoPrivacy
}

export class Service extends Context.Service<Service, Interface>()("blip/Repository") {}

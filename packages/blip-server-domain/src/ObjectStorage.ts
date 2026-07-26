import * as Context from "effect/Context"
import type * as Effect from "effect/Effect"
import type { ObjectStorageError } from "./Errors.ts"
import type { StorageConfig } from "./Model.ts"

export interface Interface {
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

export class Service extends Context.Service<Service, Interface>()(
  "blip/ObjectStorage"
) {}

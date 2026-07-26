import * as Data from "effect/Data"

export class RepositoryError extends Data.TaggedError("RepositoryError")<{ readonly cause: unknown }> {}
export class VaultError extends Data.TaggedError("VaultError")<{ readonly cause: unknown }> {}
export class ObjectStorageError extends Data.TaggedError("ObjectStorageError")<{ readonly cause: unknown }> {}

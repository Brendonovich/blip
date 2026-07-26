import { NotFound, StoragePolicy } from "@blip/server-domain"
import * as Effect from "effect/Effect"
import * as Layer from "effect/Layer"
import * as Policy from "./Policy.ts"

export const layer: Layer.Layer<StoragePolicy.Service> = Layer.succeed(StoragePolicy.Service)({
  canManageStorage: () => Policy.policy(() => Effect.succeed(true)),
  canUpload: () =>
    Policy.policy((user) =>
      user.storageConfig
        ? Effect.succeed(true)
        : Effect.fail(new NotFound({ message: "Storage is not configured" }))
    )
})

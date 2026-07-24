import * as Context from "effect/Context"
import * as Effect from "effect/Effect"
import * as Layer from "effect/Layer"
import { NotFound } from "../api.ts"
import * as Policy from "./Policy.ts"

export interface Interface {
  readonly canManageStorage: () => Policy.Policy<never, never>
  readonly canUpload: () => Policy.Policy<NotFound, never>
}

export class Service extends Context.Service<Service, Interface>()("blip/StoragePolicy") {}

export const layer: Layer.Layer<Service> = Layer.succeed(Service)({
  canManageStorage: () => Policy.policy(() => Effect.succeed(true)),
  canUpload: () =>
    Policy.policy((user) =>
      user.storageConfig
        ? Effect.succeed(true)
        : Effect.fail(new NotFound({ message: "Storage is not configured" }))
    )
})

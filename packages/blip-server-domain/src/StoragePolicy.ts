import * as Context from "effect/Context"
import type { NotFound } from "./Api.ts"
import type { Policy } from "./Policy.ts"

export interface Interface {
  readonly canManageStorage: () => Policy
  readonly canUpload: () => Policy<NotFound>
}

export class Service extends Context.Service<Service, Interface>()(
  "blip/StoragePolicy"
) {}

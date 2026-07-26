import * as Context from "effect/Context"

export interface Interface {
  readonly id: string
  readonly name: string
  readonly storageConfig: string | null
}

export class Service extends Context.Service<Service, Interface>()("blip/CurrentUser") {}

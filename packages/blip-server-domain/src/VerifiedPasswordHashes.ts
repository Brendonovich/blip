import * as Context from "effect/Context"

export interface Interface {
  readonly hashes: ReadonlyArray<string>
}

export class Service extends Context.Service<Service, Interface>()("blip/VerifiedPasswordHashes") {}

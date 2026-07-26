import * as Context from "effect/Context"
import type * as Effect from "effect/Effect"

export interface AuthIdentity {
  readonly id: string
  readonly name: string
}

export interface Interface {
  readonly verifyApiKey: (key: string) => Effect.Effect<AuthIdentity | undefined>
  readonly verifySession: (headers: Headers) => Effect.Effect<AuthIdentity | undefined>
}

export class Service extends Context.Service<Service, Interface>()("blip/Auth") {}

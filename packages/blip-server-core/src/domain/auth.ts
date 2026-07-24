import * as Context from "effect/Context"
import * as Effect from "effect/Effect"
import * as Layer from "effect/Layer"
import { Repository } from "./repository.ts"

export interface AuthIdentity {
  readonly id: string
  readonly name: string
}

export interface AuthService {
  readonly verifyApiKey: (key: string) => Effect.Effect<AuthIdentity | undefined>
  readonly verifySession: (headers: Headers) => Effect.Effect<AuthIdentity | undefined>
}

export class Auth extends Context.Service<Auth, AuthService>()("blip/Auth") {}

export const legacyLayer = Layer.effect(Auth)(Effect.gen(function*() {
  const repository = yield* Repository
  return Auth.of({
    verifyApiKey: (key) => repository.findUserByApiKeyHash(key).pipe(
      Effect.orDie,
      Effect.map((user) => user ? { id: user.id, name: user.name } : undefined)
    ),
    verifySession: () => Effect.succeed(undefined)
  })
}))

import { Auth, Repository } from "@blip/server-domain"
import * as Effect from "effect/Effect"
import * as Layer from "effect/Layer"

export const legacyLayer = Layer.effect(Auth.Service)(Effect.gen(function*() {
  const repository = yield* Repository.Service
  return Auth.Service.of({
    verifyApiKey: (key) => repository.findUserByApiKeyHash(key).pipe(
      Effect.orDie,
      Effect.map((user) => user ? { id: user.id, name: user.name } : undefined)
    ),
    verifySession: () => Effect.succeed(undefined)
  })
}))

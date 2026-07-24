import * as Context from "effect/Context"
import * as Effect from "effect/Effect"
import * as Layer from "effect/Layer"
import * as Option from "effect/Option"
import { PasswordRequired, VerifiedPasswordHashes } from "../api.ts"
import type { RepositoryError } from "./errors.ts"
import * as Policy from "./Policy.ts"
import { Repository } from "./repository.ts"

export interface Interface {
  readonly isOwner: (videoId: string) => Policy.Policy<RepositoryError, Repository>
  readonly canView: (
    videoId: string
  ) => Policy.PublicPolicy<
    RepositoryError | PasswordRequired,
    Repository
  >
}

export class Service extends Context.Service<Service, Interface>()("blip/VideosPolicy") {}

export const layer: Layer.Layer<Service, never, Repository> = Layer.effect(Service)(
  Effect.gen(function*() {
    const repository = yield* Repository

    const isOwner = (videoId: string) =>
      Policy.policy((user) =>
        repository.findVideoById(videoId).pipe(
          Effect.map((video) => !video || video.userId === user.id)
        )
      )

    const canView = (videoId: string) =>
      Policy.publicPolicy((user) =>
        Effect.gen(function*() {
          const video = yield* repository.findVideo(videoId)
          if (!video || video.status !== "complete") {
            return true
          }
          if (Option.isSome(user) && user.value.id === video.userId) {
            return true
          }
          if (video.privacy === "public") return true
          if (video.privacy === "password") {
            if (!video.passwordHash) return false
            const verified = yield* Effect.serviceOption(VerifiedPasswordHashes)
            if (Option.isSome(verified) && verified.value.hashes.includes(video.passwordHash)) {
              return true
            }
            return yield* Effect.fail(new PasswordRequired({ message: "This recording requires a password" }))
          }
          return false
        })
      )

    return Service.of({ isOwner, canView })
  })
)

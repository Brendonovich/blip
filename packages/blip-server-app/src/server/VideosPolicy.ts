import {
  PasswordRequired,
  Repository,
  VerifiedPasswordHashes,
  VideosPolicy
} from "@blip/server-domain"
import * as Effect from "effect/Effect"
import * as Layer from "effect/Layer"
import * as Option from "effect/Option"
import * as Policy from "./Policy.ts"

export const layer: Layer.Layer<VideosPolicy.Service, never, Repository.Service> = Layer.effect(VideosPolicy.Service)(
  Effect.gen(function*() {
    const repository = yield* Repository.Service

    const isOwner = (videoId: string) =>
      Policy.policy((user) =>
        repository.findVideo(videoId).pipe(
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
            const verified = yield* Effect.serviceOption(VerifiedPasswordHashes.Service)
            if (Option.isSome(verified) && verified.value.hashes.includes(video.passwordHash)) {
              return true
            }
            return yield* Effect.fail(new PasswordRequired({ message: "This recording requires a password" }))
          }
          return false
        })
      )

    return VideosPolicy.Service.of({ isOwner, canView })
  })
)

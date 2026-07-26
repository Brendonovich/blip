import * as Data from "effect/Data"
import type * as Effect from "effect/Effect"
import type { PolicyDenied } from "./Api.ts"
import type * as CurrentUser from "./CurrentUser.ts"
import type * as OptionalCurrentUser from "./OptionalCurrentUser.ts"

export type Policy<E = never, R = never> = Effect.Effect<void, PolicyDenied | E, CurrentUser.Service | R>
export type PublicPolicy<E = never, R = never> = Effect.Effect<void, PolicyDenied | E, OptionalCurrentUser.Service | R>

export class DenyAccess extends Data.TaggedError("DenyAccess")<{}> {}

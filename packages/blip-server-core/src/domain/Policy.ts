import * as Context from "effect/Context"
import * as Data from "effect/Data"
import * as Effect from "effect/Effect"
import * as Option from "effect/Option"
import { CurrentUser, type CurrentUserService, OptionalCurrentUser, PolicyDenied } from "../api.ts"

export type Policy<E = never, R = never> = Effect.Effect<
  void,
  PolicyDenied | E,
  CurrentUser | R
>

export type PublicPolicy<E = never, R = never> = Effect.Effect<
  void,
  PolicyDenied | E,
  OptionalCurrentUser | R
>

export class DenyAccess extends Data.TaggedError("DenyAccess")<{}> {}

export const policy = <E, R>(
  predicate: (user: CurrentUserService) => Effect.Effect<boolean, E | DenyAccess, R>
): Policy<E, R> =>
  Effect.flatMap(CurrentUser, (user) =>
    Effect.flatMap(
      predicate(user).pipe(
        Effect.catchTag("DenyAccess", () => Effect.succeed(false))
      ),
      (result) => (result ? Effect.void : Effect.fail(new PolicyDenied({ message: "Access denied by policy" })))
    )
  ) as Policy<E, R>

export const publicPolicy = <E, R>(
  predicate: (user: Option.Option<CurrentUserService>) => Effect.Effect<boolean, E | DenyAccess, R>
): PublicPolicy<E, R> =>
  Effect.flatMap(OptionalCurrentUser, ({ user }) =>
    Effect.flatMap(
      predicate(user).pipe(
        Effect.catchTag("DenyAccess", () => Effect.succeed(false))
      ),
      (result) => (result ? Effect.void : Effect.fail(new PolicyDenied({ message: "Access denied by policy" })))
    )
  ) as PublicPolicy<E, R>

export const withPolicy =
  <E, R>(policy: Policy<E, R>) =>
  <A, E2, R2>(self: Effect.Effect<A, E2, R2>) =>
    Effect.andThen(policy, self)

export const withPublicPolicy =
  <E, R>(policy: PublicPolicy<E, R>) =>
  <A, E2, R2>(self: Effect.Effect<A, E2, R2>) =>
    Effect.andThen(policy, self)

export const all = <E, R>(
  ...policies: readonly [Policy<E, R>, ...Array<Policy<E, R>>]
): Policy<E, R> =>
  Effect.all(policies, {
    concurrency: 1,
    discard: true
  }) as Policy<E, R>

export const any = <E, R>(
  ...policies: readonly [Policy<E, R>, ...Array<Policy<E, R>>]
): Policy<E, R> =>
  Effect.firstSuccessOf(policies) as Policy<E, R>

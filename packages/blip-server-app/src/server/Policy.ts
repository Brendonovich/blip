import {
  CurrentUser,
  DenyAccess,
  OptionalCurrentUser,
  PolicyDenied,
  type Policy,
  type PublicPolicy
} from "@blip/server-domain"
import * as Effect from "effect/Effect"
import * as Option from "effect/Option"

export { DenyAccess }
export type { Policy, PublicPolicy }

export const policy = <E, R>(
  predicate: (user: CurrentUser.Interface) => Effect.Effect<boolean, E | DenyAccess, R>
): Policy<E, R> =>
  Effect.flatMap(CurrentUser.Service, (user) =>
    Effect.flatMap(
      predicate(user).pipe(
        Effect.catchTag("DenyAccess", () => Effect.succeed(false))
      ),
      (result) => (result ? Effect.void : Effect.fail(new PolicyDenied({ message: "Access denied by policy" })))
    )
  ) as Policy<E, R>

export const publicPolicy = <E, R>(
  predicate: (user: Option.Option<CurrentUser.Interface>) => Effect.Effect<boolean, E | DenyAccess, R>
): PublicPolicy<E, R> =>
  Effect.flatMap(OptionalCurrentUser.Service, ({ user }) =>
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

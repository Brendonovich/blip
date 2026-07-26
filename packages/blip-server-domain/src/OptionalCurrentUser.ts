import * as Context from "effect/Context"
import type * as Option from "effect/Option"
import type * as CurrentUser from "./CurrentUser.ts"

export interface Interface {
  readonly user: Option.Option<CurrentUser.Interface>
}

export class Service extends Context.Service<Service, Interface>()("blip/OptionalCurrentUser") {}

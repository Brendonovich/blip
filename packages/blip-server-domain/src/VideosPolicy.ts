import * as Context from "effect/Context"
import type { PasswordRequired } from "./Api.ts"
import type { RepositoryError } from "./Errors.ts"
import type { Policy, PublicPolicy } from "./Policy.ts"
import type * as Repository from "./Repository.ts"

export interface Interface {
  readonly isOwner: (videoId: string) => Policy<RepositoryError, Repository.Service>
  readonly canView: (videoId: string) => PublicPolicy<RepositoryError | PasswordRequired, Repository.Service>
}

export class Service extends Context.Service<Service, Interface>()(
  "blip/VideosPolicy"
) {}

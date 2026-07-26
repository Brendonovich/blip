import * as Context from "effect/Context"
import type * as Effect from "effect/Effect"
import type { VaultError } from "./Errors.ts"
import type { StorageConfig } from "./Model.ts"

export interface Interface {
  readonly encrypt: (config: StorageConfig) => Effect.Effect<string, VaultError>
  readonly decrypt: (value: string) => Effect.Effect<StorageConfig, VaultError>
  readonly encryptString: (value: string) => Effect.Effect<string, VaultError>
  readonly decryptString: (value: string) => Effect.Effect<string, VaultError>
}

export class Service extends Context.Service<Service, Interface>()("blip/Vault") {}

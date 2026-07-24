import * as Context from "effect/Context"
import * as Effect from "effect/Effect"
import * as Layer from "effect/Layer"
import * as Schema from "effect/Schema"
import { VaultError } from "./errors.ts"
import { StorageConfig, type StorageConfig as StorageConfigValue } from "./model.ts"

export interface VaultService {
  readonly encrypt: (config: StorageConfigValue) => Effect.Effect<string, VaultError>
  readonly decrypt: (value: string) => Effect.Effect<StorageConfigValue, VaultError>
  readonly encryptString: (value: string) => Effect.Effect<string, VaultError>
  readonly decryptString: (value: string) => Effect.Effect<string, VaultError>
}

export class Vault extends Context.Service<Vault, VaultService>()("blip/Vault") {}

const base64UrlEncode = (bytes: Uint8Array) => {
  let binary = ""
  for (const byte of bytes) binary += String.fromCharCode(byte)
  return btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "")
}

const base64UrlDecode = (value: string) => {
  const standard = value.replaceAll("-", "+").replaceAll("_", "/")
  const binary = atob(standard.padEnd(Math.ceil(standard.length / 4) * 4, "="))
  return Uint8Array.from(binary, (character) => character.charCodeAt(0))
}

export const layer = (encodedKey: string) => Layer.effect(Vault)(
  Effect.tryPromise({
    try: async () => {
      const keyBytes = base64UrlDecode(encodedKey)
      if (keyBytes.length !== 32) throw new Error("BLIP_ENCRYPTION_KEY must contain 32 bytes")
      const key = await crypto.subtle.importKey("raw", keyBytes, "AES-GCM", false, ["encrypt", "decrypt"])
      const encoder = new TextEncoder()
      const decoder = new TextDecoder()
      const encryptString = (value: string) => Effect.tryPromise({
        try: async () => {
          const iv = crypto.getRandomValues(new Uint8Array(12))
          const encrypted = await crypto.subtle.encrypt(
            { name: "AES-GCM", iv },
            key,
            encoder.encode(value)
          )
          return `v1.${base64UrlEncode(iv)}.${base64UrlEncode(new Uint8Array(encrypted))}`
        },
        catch: (cause) => new VaultError({ cause })
      })
      const decryptString = (value: string) => Effect.tryPromise({
        try: async () => {
          const [version, encodedIv, encodedPayload] = value.split(".")
          if (version !== "v1" || !encodedIv || !encodedPayload) throw new Error("Invalid encrypted payload")
          const decrypted = await crypto.subtle.decrypt(
            { name: "AES-GCM", iv: base64UrlDecode(encodedIv) },
            key,
            base64UrlDecode(encodedPayload)
          )
          return decoder.decode(decrypted)
        },
        catch: (cause) => new VaultError({ cause })
      })
      return Vault.of({
        encrypt: (config) => encryptString(JSON.stringify(config)),
        decrypt: (value) => decryptString(value).pipe(
          Effect.map((text) => Schema.decodeUnknownSync(StorageConfig)(JSON.parse(text)))
        ),
        encryptString,
        decryptString
      })
    },
    catch: (cause) => new VaultError({ cause })
  }).pipe(Effect.orDie)
)

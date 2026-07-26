import { apiKey } from "@better-auth/api-key"
import { user, users } from "@blip/server-app/schema"
import { Auth } from "@blip/server-app/Server"
import { betterAuth } from "better-auth"
import { eq } from "drizzle-orm"
import { drizzle } from "drizzle-orm/d1"
import * as Effect from "effect/Effect"
import * as Layer from "effect/Layer"

export interface AuthConfig {
  readonly baseUrl: string
  readonly secret: string
  readonly githubClientId: string
  readonly githubClientSecret: string
}

export const makeAuth = (database: D1Database, config: AuthConfig) => betterAuth({
  appName: "Blip",
  baseURL: config.baseUrl,
  secret: config.secret,
  database,
  trustedOrigins: [config.baseUrl, "http://localhost:1337"],
  socialProviders: {
    github: {
      clientId: config.githubClientId,
      clientSecret: config.githubClientSecret
    }
  },
  plugins: [apiKey({
    defaultPrefix: "blip_",
    requireName: true,
    rateLimit: { enabled: false }
  })]
})

export type BlipAuth = ReturnType<typeof makeAuth>

export const authLayer = (
  database: D1Database,
  config: Omit<AuthConfig, "secret">,
  secret: string
) => Layer.succeed(Auth.Service)({
  verifyApiKey: (key) => Effect.gen(function*() {
    const auth = makeAuth(database, { ...config, secret })
    const db = drizzle(database)
    return yield* Effect.promise(async () => {
    try {
      const result = await auth.api.verifyApiKey({ body: { key } })
      if (result.valid && result.key) {
        const userRecord = await db.select({ name: user.name }).from(user)
          .where(eq(user.id, result.key.referenceId)).get()
        if (userRecord) return { id: result.key.referenceId, name: userRecord.name }
      }
      const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(key))
      const hash = Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, "0")).join("")
      const legacyUser = await db.select({ id: users.id, name: users.name }).from(users)
        .where(eq(users.apiKeyHash, hash)).get()
      if (legacyUser) return legacyUser
    } catch {}
    })
  }),
  verifySession: (headers) => Effect.gen(function*() {
    const auth = makeAuth(database, { ...config, secret })
    return yield* Effect.promise(async () => {
    try {
      const session = await auth.api.getSession({ headers })
      if (session) return { id: session.user.id, name: session.user.name }
    } catch {}
    })
  })
})

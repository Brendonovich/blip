import * as Cloudflare from "cloudflare:workers"

interface Env {
  readonly DB: D1Database
  readonly BETTER_AUTH_SECRET: string
  readonly BLIP_ENCRYPTION_KEY: string
  readonly BLIP_PUBLIC_ORIGIN: string
  readonly BLIP_SETUP_TOKEN: string
  readonly GITHUB_CLIENT_ID: string
  readonly GITHUB_CLIENT_SECRET: string
}

export const env = new Proxy({} as Env, {
  get(_, property) {
    const val = Cloudflare.env[property as keyof typeof Cloudflare.env]
    if (val !== undefined && val !== "undefined") return val
    if (property === "BLIP_PUBLIC_ORIGIN") return "http://localhost:1337"
    return val
  }
})

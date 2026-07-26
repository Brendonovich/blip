declare module "@blip/server-runtime" {
  import type { APIHandler } from "@solidjs/start/server"

  export const ensureInMemoryApi: () => void
  export const handleApi: APIHandler
  export const handleAuth: APIHandler
}

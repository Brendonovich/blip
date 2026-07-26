import { solidStart } from "@solidjs/start/config"
import { nitro } from "nitro/vite"
import { fileURLToPath } from "node:url"
import { defineConfig } from "vite"

export default defineConfig({
  ssr: {
    external: ["tslib"]
  },
  resolve: {
    alias: {
      "@blip/server-runtime": fileURLToPath(new URL("./src/server.ts", import.meta.url))
    }
  },
  plugins: [
    solidStart({
      ssr: true,
      appRoot: "../../packages/blip-server-app/src"
    } as Parameters<typeof solidStart>[0]),
    nitro({ preset: "node-server" })
  ]
})

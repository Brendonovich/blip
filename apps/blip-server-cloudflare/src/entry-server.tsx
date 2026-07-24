import { createHandler, StartServer } from "@solidjs/start/server"
import { ensureInMemoryApi } from "./server.ts"

export default createHandler(() => {
  ensureInMemoryApi()
  return (
    <StartServer
      document={({ assets, children, scripts }) => (
        <html lang="en">
          <head>
            <meta charset="UTF-8" />
            <meta name="viewport" content="width=device-width, initial-scale=1.0" />
            <meta name="theme-color" content="#171713" />
            <title>Blip</title>
            {assets}
          </head>
          <body>
            <div id="app">{children}</div>
            {scripts}
          </body>
        </html>
      )}
    />
  )
})

import { D1Client } from "@effect/sql-d1"
import * as Drizzle from "drizzle-orm/effect-d1"
import {
  objectStorageLayer,
  provideServerServices,
  serverLayer,
  vaultLayer
} from "@blip/server-app/Server"
import { repositoryLayer } from "@blip/server-app/Repository"
import type { APIHandler } from "@solidjs/start/server"
import { setInMemoryAuthHandler, setInMemoryHttpClient } from "@blip/server-app/in-memory-api"
import * as Effect from "effect/Effect"
import * as FileSystem from "effect/FileSystem"
import * as Layer from "effect/Layer"
import * as Path from "effect/Path"
import * as Etag from "effect/unstable/http/Etag"
import * as HttpClient from "effect/unstable/http/HttpClient"
import * as HttpClientResponse from "effect/unstable/http/HttpClientResponse"
import * as HttpPlatform from "effect/unstable/http/HttpPlatform"
import * as HttpRouter from "effect/unstable/http/HttpRouter"
import * as HttpServerRequest from "effect/unstable/http/HttpServerRequest"
import { authLayer, makeAuth } from "./auth.ts"
import { env } from "./env.ts"

const HttpPlatformStub = Layer.succeed(HttpPlatform.HttpPlatform, {
  fileResponse: () => Effect.die("HttpPlatform.fileResponse is not supported"),
  fileWebResponse: () => Effect.die("HttpPlatform.fileWebResponse is not supported")
})

let apiHandler: ((request: Request) => Promise<Response>) | undefined

const getApiHandler = () => {
  if (apiHandler) return apiHandler

  const authConfig = {
    baseUrl: new URL(env.BLIP_PUBLIC_ORIGIN).origin,
    githubClientId: env.GITHUB_CLIENT_ID,
    githubClientSecret: env.GITHUB_CLIENT_SECRET
  }
  const SqlLive = D1Client.layer({ db: env.DB })
  const RepositoryLive = repositoryLayer(Drizzle.makeWithDefaults({})).pipe(Layer.provide(SqlLive))
  const ServicesLive = Layer.mergeAll(
    RepositoryLive,
    objectStorageLayer,
    vaultLayer(env.BLIP_ENCRYPTION_KEY),
    authLayer(env.DB, authConfig, env.BETTER_AUTH_SECRET)
  )
  const AppLive = serverLayer(env.BLIP_SETUP_TOKEN || "", authConfig.baseUrl).pipe(
    Layer.provide(ServicesLive),
    provideServerServices(ServicesLive),
    Layer.provide([Etag.layer, FileSystem.layerNoop({}), HttpPlatformStub, Path.layer])
  )

  const handler = HttpRouter.toWebHandler(AppLive as any).handler as (request: Request) => Promise<Response>
  const inMemoryClient = HttpClient.make((request) =>
    Effect.gen(function*() {
      const webRequest = yield* HttpServerRequest.toWeb(HttpServerRequest.fromClientRequest(request))
      const response = yield* Effect.promise(() => handler(webRequest))
      return HttpClientResponse.fromWeb(request, response)
    }).pipe(Effect.orDie)
  )
  setInMemoryHttpClient(inMemoryClient)
  setInMemoryAuthHandler((req) => handleAuth({ request: req } as any))

  apiHandler = handler
  return handler
}

export const ensureInMemoryApi = () => {
  getApiHandler()
}

export const handleApi: APIHandler = ({ request }) => getApiHandler()(request)

export const handleAuth: APIHandler = ({ request }) => {
  const auth = makeAuth(env.DB, {
    baseUrl: new URL(env.BLIP_PUBLIC_ORIGIN).origin,
    secret: env.BETTER_AUTH_SECRET,
    githubClientId: env.GITHUB_CLIENT_ID,
    githubClientSecret: env.GITHUB_CLIENT_SECRET
  })
  return auth.handler(request)
}

import * as SqliteClient from "@effect/sql-sqlite-node/SqliteClient"
import {
  objectStorageLayer,
  provideServerServices,
  serverLayer,
  vaultLayer
} from "@blip/server-app/Server"
import { setInMemoryAuthHandler, setInMemoryHttpClient } from "@blip/server-app/in-memory-api"
import { repositoryLayer } from "@blip/server-app/Repository"
import type { APIHandler } from "@solidjs/start/server"
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
import { drizzle } from "drizzle-orm/node-sqlite"
import { migrate } from "drizzle-orm/node-sqlite/migrator"
import * as Drizzle from "drizzle-orm/effect-sqlite-node"
import { existsSync, mkdirSync } from "node:fs"
import { dirname } from "node:path"
import { DatabaseSync } from "node:sqlite"
import { authLayer, makeAuth } from "./auth.ts"

const required = (name: string) => {
  const value = process.env[name]
  if (!value) throw new Error(`${name} is required`)
  return value
}

const databasePath = process.env.DATABASE_PATH || "./data/blip.sqlite"
const baseUrl = new URL(process.env.BLIP_PUBLIC_ORIGIN || `http://localhost:${process.env.PORT || "3000"}`).origin
const config = {
  baseUrl,
  secret: required("BETTER_AUTH_SECRET"),
  githubClientId: required("GITHUB_CLIENT_ID"),
  githubClientSecret: required("GITHUB_CLIENT_SECRET")
}

mkdirSync(dirname(databasePath), { recursive: true })
const database = new DatabaseSync(databasePath)
database.exec("PRAGMA foreign_keys = ON")
const migrationsFolder = process.env.DRIZZLE_MIGRATIONS_PATH || [
  "./migrations",
  "../../packages/blip-server-app/migrations",
  "./packages/blip-server-app/migrations"
].find(existsSync)
if (!migrationsFolder) throw new Error("Drizzle migrations directory not found")
migrate(drizzle({ client: database }), {
  migrationsFolder
})

const HttpPlatformStub = Layer.succeed(HttpPlatform.HttpPlatform, {
  fileResponse: () => Effect.die("HttpPlatform.fileResponse is not supported"),
  fileWebResponse: () => Effect.die("HttpPlatform.fileWebResponse is not supported")
})

let apiHandler: ((request: Request) => Promise<Response>) | undefined

const getApiHandler = () => {
  if (apiHandler) return apiHandler

  const SqlLive = SqliteClient.layer({ filename: databasePath })
  const RepositoryLive = repositoryLayer(Drizzle.makeWithDefaults()).pipe(Layer.provide(SqlLive))
  const ServicesLive = Layer.mergeAll(
    RepositoryLive,
    objectStorageLayer,
    vaultLayer(required("BLIP_ENCRYPTION_KEY")),
    authLayer(database, config)
  )
  const AppLive = serverLayer(process.env.BLIP_SETUP_TOKEN || "", baseUrl).pipe(
    Layer.provide(ServicesLive),
    provideServerServices(ServicesLive),
    Layer.provide([Etag.layer, FileSystem.layerNoop({}), HttpPlatformStub, Path.layer])
  )
  const handler = HttpRouter.toWebHandler(AppLive as any).handler as (request: Request) => Promise<Response>
  const initializedHandler = async (request: Request) => {
    return handler(request)
  }
  const inMemoryClient = HttpClient.make((request) => Effect.gen(function*() {
    const webRequest = yield* HttpServerRequest.toWeb(HttpServerRequest.fromClientRequest(request))
    const response = yield* Effect.promise(() => initializedHandler(webRequest))
    return HttpClientResponse.fromWeb(request, response)
  }).pipe(Effect.orDie))

  setInMemoryHttpClient(inMemoryClient)
  setInMemoryAuthHandler((request) => handleAuth({ request } as any))
  apiHandler = initializedHandler
  return initializedHandler
}

export const ensureInMemoryApi = () => {
  getApiHandler()
}

export const handleApi: APIHandler = ({ request }) => getApiHandler()(request)

export const handleAuth: APIHandler = ({ request }) => makeAuth(database, config).handler(request)

import { NodeHttpServer, NodeRuntime } from "@effect/platform-node"
import * as SqliteClient from "@effect/sql-sqlite-node/SqliteClient"
import * as Config from "effect/Config"
import * as Effect from "effect/Effect"
import * as Layer from "effect/Layer"
import * as Redacted from "effect/Redacted"
import * as HttpRouter from "effect/unstable/http/HttpRouter"
import { createServer } from "node:http"
import { mkdirSync } from "node:fs"
import { dirname } from "node:path"
import {
  initializeRepository,
  legacyAuthLayer,
  objectStorageLayer,
  provideServerServices,
  repositoryLayer,
  serverLayer,
  StoragePolicy,
  vaultLayer,
  VideosPolicy
} from "@blip/server-core"

const main = Effect.gen(function*() {
  const host = yield* Config.string("HOST").pipe(Config.withDefault("0.0.0.0"))
  const port = yield* Config.port("PORT").pipe(Config.withDefault(3000))
  const databasePath = yield* Config.string("DATABASE_PATH").pipe(Config.withDefault("./data/blip.sqlite"))
  const encryptionKey = yield* Config.redacted("BLIP_ENCRYPTION_KEY")
  const setupToken = yield* Config.redacted("BLIP_SETUP_TOKEN").pipe(Config.withDefault(Redacted.make("")))
  const publicOrigin = yield* Config.url("BLIP_PUBLIC_ORIGIN")

  yield* Effect.sync(() => mkdirSync(dirname(databasePath), { recursive: true }))

  const SqlLive = SqliteClient.layer({ filename: databasePath })
  const DataLive = Layer.mergeAll(
    repositoryLayer,
    Layer.effectDiscard(initializeRepository)
  ).pipe(Layer.provide(SqlLive))
  const ServicesLive = Layer.mergeAll(
    DataLive,
    legacyAuthLayer.pipe(Layer.provide(DataLive)),
    objectStorageLayer,
    vaultLayer(Redacted.value(encryptionKey)),
    StoragePolicy.layer,
    VideosPolicy.layer.pipe(Layer.provide(DataLive))
  )
  const AppLive = serverLayer(Redacted.value(setupToken), publicOrigin.origin).pipe(
    Layer.provide(ServicesLive),
    provideServerServices(ServicesLive)
  )
  const ServerLive = HttpRouter.serve(AppLive).pipe(
    Layer.provide(NodeHttpServer.layer(createServer, {
      host,
      port,
      gracefulShutdownTimeout: "10 seconds"
    }))
  )

  yield* Layer.launch(ServerLive)
})

NodeRuntime.runMain(main)

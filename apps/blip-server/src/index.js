"use strict";
var __createBinding = (this && this.__createBinding) || (Object.create ? (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    var desc = Object.getOwnPropertyDescriptor(m, k);
    if (!desc || ("get" in desc ? !m.__esModule : desc.writable || desc.configurable)) {
      desc = { enumerable: true, get: function() { return m[k]; } };
    }
    Object.defineProperty(o, k2, desc);
}) : (function(o, m, k, k2) {
    if (k2 === undefined) k2 = k;
    o[k2] = m[k];
}));
var __setModuleDefault = (this && this.__setModuleDefault) || (Object.create ? (function(o, v) {
    Object.defineProperty(o, "default", { enumerable: true, value: v });
}) : function(o, v) {
    o["default"] = v;
});
var __importStar = (this && this.__importStar) || (function () {
    var ownKeys = function(o) {
        ownKeys = Object.getOwnPropertyNames || function (o) {
            var ar = [];
            for (var k in o) if (Object.prototype.hasOwnProperty.call(o, k)) ar[ar.length] = k;
            return ar;
        };
        return ownKeys(o);
    };
    return function (mod) {
        if (mod && mod.__esModule) return mod;
        var result = {};
        if (mod != null) for (var k = ownKeys(mod), i = 0; i < k.length; i++) if (k[i] !== "default") __createBinding(result, mod, k[i]);
        __setModuleDefault(result, mod);
        return result;
    };
})();
Object.defineProperty(exports, "__esModule", { value: true });
const platform_node_1 = require("@effect/platform-node");
const SqliteClient = __importStar(require("@effect/sql-sqlite-node/SqliteClient"));
const Config = __importStar(require("effect/Config"));
const Effect = __importStar(require("effect/Effect"));
const Layer = __importStar(require("effect/Layer"));
const Redacted = __importStar(require("effect/Redacted"));
const HttpRouter = __importStar(require("effect/unstable/http/HttpRouter"));
const node_http_1 = require("node:http");
const node_fs_1 = require("node:fs");
const node_path_1 = require("node:path");
const server_core_1 = require("@blip/server-core");
const main = Effect.gen(function* () {
    const host = yield* Config.string("HOST").pipe(Config.withDefault("0.0.0.0"));
    const port = yield* Config.port("PORT").pipe(Config.withDefault(3000));
    const databasePath = yield* Config.string("DATABASE_PATH").pipe(Config.withDefault("./data/blip.sqlite"));
    const encryptionKey = yield* Config.redacted("BLIP_ENCRYPTION_KEY");
    const publicOrigin = yield* Config.url("BLIP_PUBLIC_ORIGIN");
    yield* Effect.sync(() => (0, node_fs_1.mkdirSync)((0, node_path_1.dirname)(databasePath), { recursive: true }));
    const SqlLive = SqliteClient.layer({ filename: databasePath });
    const DataLive = Layer.mergeAll(server_core_1.repositoryLayer, Layer.effectDiscard(server_core_1.initializeRepository)).pipe(Layer.provide(SqlLive));
    const ServicesLive = Layer.mergeAll(DataLive, server_core_1.legacyAuthLayer.pipe(Layer.provide(DataLive)), server_core_1.objectStorageLayer, (0, server_core_1.vaultLayer)(Redacted.value(encryptionKey)));
    const AppLive = (0, server_core_1.serverLayer)(publicOrigin.origin).pipe(Layer.provide(ServicesLive), (0, server_core_1.provideServerServices)(ServicesLive));
    const ServerLive = HttpRouter.serve(AppLive).pipe(Layer.provide(platform_node_1.NodeHttpServer.layer(node_http_1.createServer, {
        host,
        port,
        gracefulShutdownTimeout: "10 seconds"
    })));
    yield* Layer.launch(ServerLive);
});
platform_node_1.NodeRuntime.runMain(main);

import { Repository, RepositoryError } from "@blip/server-domain"
import { and, desc, eq, isNull } from "drizzle-orm"
import type { SQLiteEffectDatabase } from "drizzle-orm/sqlite-core/effect"
import * as Effect from "effect/Effect"
import * as Layer from "effect/Layer"
import { users, videos } from "./schema.ts"

type Database = SQLiteEffectDatabase<any, any, any>

const databaseError = <A>(effect: Effect.Effect<A, unknown, any>): Effect.Effect<A, RepositoryError> =>
	Effect.catchCause(effect, (cause) => Effect.fail(new RepositoryError({ cause }))) as Effect.Effect<A, RepositoryError>

export const repositoryLayer = <R>(database: Effect.Effect<Database, never, R>) =>
	Layer.effect(Repository.Service)(Effect.map(database, (db) => Repository.Service.of({
		createUser: (user, createdAt) => databaseError(db.insert(users).values({ ...user, createdAt })).pipe(Effect.asVoid),
		ensureUser: (id, name, createdAt) => databaseError(Effect.gen(function*() {
			yield* db.insert(users).values({ id, name, apiKeyHash: `better_auth:${id}`, storageConfig: null, createdAt })
				.onConflictDoUpdate({ target: users.id, set: { name } })
			return (yield* db.select().from(users).where(eq(users.id, id)).get())!
		})),
		findUserByApiKeyHash: (apiKey) => databaseError(Effect.promise(async () => {
			const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(apiKey))
			return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, "0")).join("")
		})).pipe(Effect.flatMap((hash) => databaseError(db.select().from(users).where(eq(users.apiKeyHash, hash)).get()))),
		setStorageConfig: (userId, storageConfig) => databaseError(db.update(users).set({ storageConfig }).where(eq(users.id, userId))).pipe(Effect.asVoid),
		createVideo: (video, createdAt) => databaseError(db.insert(videos).values({ ...video, createdAt })).pipe(Effect.asVoid),
		findOwnedVideo: (userId, id) => databaseError(db.select().from(videos).where(and(eq(videos.id, id), eq(videos.userId, userId))).get()),
		findVideo: (id) => databaseError(db.select().from(videos).where(eq(videos.id, id)).get()),
		listOwnedVideos: (userId) => databaseError(db.select({
			id: videos.id,
			name: videos.name,
			objectKey: videos.objectKey,
			status: videos.status,
			createdAt: videos.createdAt,
			privacy: videos.privacy
		}).from(videos).where(and(eq(videos.userId, userId), isNull(videos.archivedAt))).orderBy(desc(videos.createdAt))),
		setVideoName: (id, name) => databaseError(db.update(videos).set({ name }).where(eq(videos.id, id))).pipe(Effect.asVoid),
		setVideoPrivacy: (id, privacy, passwordHash) => databaseError(db.update(videos).set({ privacy, passwordHash }).where(eq(videos.id, id))).pipe(Effect.asVoid),
		archiveVideo: (id, archivedAt) => databaseError(db.update(videos).set({ archivedAt }).where(eq(videos.id, id))).pipe(Effect.asVoid),
		markVideoComplete: (id) => databaseError(db.update(videos).set({ status: "complete" }).where(eq(videos.id, id))).pipe(Effect.asVoid),
		deleteVideo: (id) => databaseError(db.delete(videos).where(eq(videos.id, id))).pipe(Effect.asVoid)
	} satisfies Repository.Interface)))

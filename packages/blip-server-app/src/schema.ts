import { sql } from "drizzle-orm";
import {
	check,
	customType,
	index,
	integer,
	sqliteTable,
	text,
} from "drizzle-orm/sqlite-core";

const date = customType<{ data: Date }>({
	dataType: () => "date",
});

export const users = sqliteTable("users", {
	id: text().primaryKey(),
	name: text().notNull(),
	apiKeyHash: text("api_key_hash").notNull().unique(),
	storageConfig: text("storage_config"),
	createdAt: text("created_at").notNull(),
});

export const videos = sqliteTable(
	"videos",
	{
		id: text().primaryKey(),
		userId: text("user_id")
			.notNull()
			.references(() => users.id, { onDelete: "cascade" }),
		name: text().notNull(),
		objectKey: text("object_key").notNull(),
		uploadId: text("upload_id").notNull(),
		storageConfig: text("storage_config").notNull(),
		status: text({ enum: ["uploading", "complete"] }).notNull(),
		privacy: text({ enum: ["public", "password", "private"] }).notNull().default("public"),
		passwordHash: text("password_hash"),
		archivedAt: text("archived_at"),
		createdAt: text("created_at").notNull(),
	},
	(table) => [
		index("videos_user_id").on(table.userId),
		check("videos_status_check", sql`${table.status} IN ('uploading', 'complete')`),
		check(
			"videos_privacy_check",
			sql`${table.privacy} IN ('public', 'password', 'private')`,
		),
	],
);

export const user = sqliteTable("user", {
	id: text().primaryKey(),
	name: text().notNull(),
	email: text().notNull().unique(),
	emailVerified: integer().notNull(),
	image: text(),
	createdAt: date().notNull(),
	updatedAt: date().notNull(),
});

export const session = sqliteTable(
	"session",
	{
		id: text().primaryKey(),
		expiresAt: date().notNull(),
		token: text().notNull().unique(),
		createdAt: date().notNull(),
		updatedAt: date().notNull(),
		ipAddress: text(),
		userAgent: text(),
		userId: text()
			.notNull()
			.references(() => user.id, { onDelete: "cascade" }),
	},
	(table) => [index("session_userId_idx").on(table.userId)],
);

export const account = sqliteTable(
	"account",
	{
		id: text().primaryKey(),
		accountId: text().notNull(),
		providerId: text().notNull(),
		userId: text()
			.notNull()
			.references(() => user.id, { onDelete: "cascade" }),
		accessToken: text(),
		refreshToken: text(),
		idToken: text(),
		accessTokenExpiresAt: date(),
		refreshTokenExpiresAt: date(),
		scope: text(),
		password: text(),
		createdAt: date().notNull(),
		updatedAt: date().notNull(),
	},
	(table) => [index("account_userId_idx").on(table.userId)],
);

export const verification = sqliteTable(
	"verification",
	{
		id: text().primaryKey(),
		identifier: text().notNull(),
		value: text().notNull(),
		expiresAt: date().notNull(),
		createdAt: date().notNull(),
		updatedAt: date().notNull(),
	},
	(table) => [index("verification_identifier_idx").on(table.identifier)],
);

export const apiKey = sqliteTable(
	"apikey",
	{
		id: text().primaryKey(),
		configId: text().notNull(),
		name: text(),
		start: text(),
		referenceId: text().notNull(),
		prefix: text(),
		key: text().notNull(),
		refillInterval: integer(),
		refillAmount: integer(),
		lastRefillAt: date(),
		enabled: integer(),
		rateLimitEnabled: integer(),
		rateLimitTimeWindow: integer(),
		rateLimitMax: integer(),
		requestCount: integer(),
		remaining: integer(),
		lastRequest: date(),
		expiresAt: date(),
		createdAt: date().notNull(),
		updatedAt: date().notNull(),
		permissions: text(),
		metadata: text(),
	},
	(table) => [
		index("apikey_configId_idx").on(table.configId),
		index("apikey_referenceId_idx").on(table.referenceId),
		index("apikey_key_idx").on(table.key),
	],
);

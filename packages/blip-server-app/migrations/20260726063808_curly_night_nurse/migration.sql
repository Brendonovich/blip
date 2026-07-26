CREATE TABLE `account` (
	`id` text PRIMARY KEY,
	`accountId` text NOT NULL,
	`providerId` text NOT NULL,
	`userId` text NOT NULL,
	`accessToken` text,
	`refreshToken` text,
	`idToken` text,
	`accessTokenExpiresAt` date,
	`refreshTokenExpiresAt` date,
	`scope` text,
	`password` text,
	`createdAt` date NOT NULL,
	`updatedAt` date NOT NULL,
	CONSTRAINT `fk_account_userId_user_id_fk` FOREIGN KEY (`userId`) REFERENCES `user`(`id`) ON DELETE CASCADE
);
--> statement-breakpoint
CREATE TABLE `apikey` (
	`id` text PRIMARY KEY,
	`configId` text NOT NULL,
	`name` text,
	`start` text,
	`referenceId` text NOT NULL,
	`prefix` text,
	`key` text NOT NULL,
	`refillInterval` integer,
	`refillAmount` integer,
	`lastRefillAt` date,
	`enabled` integer,
	`rateLimitEnabled` integer,
	`rateLimitTimeWindow` integer,
	`rateLimitMax` integer,
	`requestCount` integer,
	`remaining` integer,
	`lastRequest` date,
	`expiresAt` date,
	`createdAt` date NOT NULL,
	`updatedAt` date NOT NULL,
	`permissions` text,
	`metadata` text
);
--> statement-breakpoint
CREATE TABLE `session` (
	`id` text PRIMARY KEY,
	`expiresAt` date NOT NULL,
	`token` text NOT NULL UNIQUE,
	`createdAt` date NOT NULL,
	`updatedAt` date NOT NULL,
	`ipAddress` text,
	`userAgent` text,
	`userId` text NOT NULL,
	CONSTRAINT `fk_session_userId_user_id_fk` FOREIGN KEY (`userId`) REFERENCES `user`(`id`) ON DELETE CASCADE
);
--> statement-breakpoint
CREATE TABLE `user` (
	`id` text PRIMARY KEY,
	`name` text NOT NULL,
	`email` text NOT NULL UNIQUE,
	`emailVerified` integer NOT NULL,
	`image` text,
	`createdAt` date NOT NULL,
	`updatedAt` date NOT NULL
);
--> statement-breakpoint
CREATE TABLE `users` (
	`id` text PRIMARY KEY,
	`name` text NOT NULL,
	`api_key_hash` text NOT NULL UNIQUE,
	`storage_config` text,
	`created_at` text NOT NULL
);
--> statement-breakpoint
CREATE TABLE `verification` (
	`id` text PRIMARY KEY,
	`identifier` text NOT NULL,
	`value` text NOT NULL,
	`expiresAt` date NOT NULL,
	`createdAt` date NOT NULL,
	`updatedAt` date NOT NULL
);
--> statement-breakpoint
CREATE TABLE `videos` (
	`id` text PRIMARY KEY,
	`user_id` text NOT NULL,
	`name` text NOT NULL,
	`object_key` text NOT NULL,
	`upload_id` text NOT NULL,
	`storage_config` text NOT NULL,
	`status` text NOT NULL,
	`privacy` text DEFAULT 'public' NOT NULL,
	`password_hash` text,
	`archived_at` text,
	`created_at` text NOT NULL,
	CONSTRAINT `fk_videos_user_id_users_id_fk` FOREIGN KEY (`user_id`) REFERENCES `users`(`id`) ON DELETE CASCADE,
	CONSTRAINT "videos_status_check" CHECK("status" IN ('uploading', 'complete')),
	CONSTRAINT "videos_privacy_check" CHECK("privacy" IN ('public', 'password', 'private'))
);
--> statement-breakpoint
CREATE INDEX `account_userId_idx` ON `account` (`userId`);--> statement-breakpoint
CREATE INDEX `apikey_configId_idx` ON `apikey` (`configId`);--> statement-breakpoint
CREATE INDEX `apikey_referenceId_idx` ON `apikey` (`referenceId`);--> statement-breakpoint
CREATE INDEX `apikey_key_idx` ON `apikey` (`key`);--> statement-breakpoint
CREATE INDEX `session_userId_idx` ON `session` (`userId`);--> statement-breakpoint
CREATE INDEX `verification_identifier_idx` ON `verification` (`identifier`);--> statement-breakpoint
CREATE INDEX `videos_user_id` ON `videos` (`user_id`);

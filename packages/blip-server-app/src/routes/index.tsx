import { For, Show, Suspense, createEffect, createSignal } from "solid-js";
import {
	createMutation,
	createQuery,
	queryOptions,
} from "@tanstack/solid-query";
import { client, runApi } from "../ApiClient.ts";
import { authClient } from "../auth-client.ts";
import { Effect } from "effect";

const emptyStorage = {
	endpoint: "https://s3.amazonaws.com",
	region: "us-east-1",
	bucket: "",
	accessKeyId: "",
	secretAccessKey: "",
	forcePathStyle: false,
};

const sessionQueryOpts = queryOptions({
	queryKey: ["session"],
	queryFn: () => authClient.getSession().then((res) => res.data),
	deferStream: true,
});

const storageQueryOpts = queryOptions({
	queryKey: ["storage"],
	queryFn: async () => {
		try {
			const value = await runApi(client.storage.get());
			return { exists: true, data: value } as const;
		} catch (error) {
			if (
				error &&
				typeof error === "object" &&
				"_tag" in error &&
				error._tag === "NotFound"
			) {
				return { exists: false, data: undefined } as const;
			}
			throw error;
		}
	},
	deferStream: true,
});

const keysQueryOpts = queryOptions({
	queryKey: ["keys"],
	queryFn: async () => {
		const { data, error } = await authClient.apiKey.list({
			query: { limit: 100 },
		});
		if (error) throw new Error(error.message);
		return (data?.apiKeys ?? []) as Array<Record<string, unknown>>;
	},
});

const recordingsQueryOpts = queryOptions({
	queryKey: ["recordings"],
	queryFn: () => runApi(client.uploads.list()),
});

export default function Dashboard() {
	const sessionQuery = createQuery(() => sessionQueryOpts);
	const storageQuery = createQuery(() => storageQueryOpts);

	return (
		<Show when={sessionQuery.data?.user} fallback={<SignIn />}>
			<main class="shell">
				<Show
					when={storageQuery.data?.exists && storageQuery.data.data}
					fallback={<OnboardingConnectStorage />}
				>
					<TopBar />

					<div class="dashboard-grid">
						<RecordingsPanel />

						<Sidebar />
					</div>
				</Show>
			</main>
		</Show>
	);
}

function TopBar() {
	const sessionQuery = createQuery(() => sessionQueryOpts);

	return (
		<header class="topbar">
			<a class="brand" href="/">
				<span class="signal" />
				BLIP
			</a>
			<Show when={sessionQuery.data?.user}>
				{(user) => (
					<div class="identity">
						<span>{user()?.name}</span>
						<button
							class="text-button"
							onClick={async () => {
								await authClient.signOut();
								void sessionQuery.refetch();
							}}
						>
							Sign out
						</button>
					</div>
				)}
			</Show>
		</header>
	);
}

function OnboardingConnectStorage() {
	const storageQuery = createQuery(() => storageQueryOpts);
	const [storage, setStorage] = createSignal({ ...emptyStorage });

	const saveStorageMutation = createMutation(() => ({
		mutationFn: async (val: typeof emptyStorage) => {
			await runApi(client.storage.set({ payload: val }));
		},
		onSuccess: () => {
			setStorage((current) => ({ ...current, secretAccessKey: "" }));
			void storageQuery.refetch();
		},
	}));

	const storageStatus = () => {
		if (saveStorageMutation.isPending) return "Testing connection...";
		if (saveStorageMutation.isError)
			return saveStorageMutation.error instanceof Error
				? saveStorageMutation.error.message
				: "Could not connect storage";
		if (saveStorageMutation.isSuccess) return "Storage connected";
		if (storageQuery.isError)
			return storageQuery.error instanceof Error
				? storageQuery.error.message
				: "Could not load storage";
		return "";
	};

	return (
		<section class="panel storage-setup-panel">
			<div class="panel-heading">
				<div>
					<h2>Connect an S3 bucket</h2>
				</div>
			</div>
			<form
				onSubmit={(event) => {
					event.preventDefault();
					saveStorageMutation.mutate(storage());
				}}
			>
				<label class="wide">
					Endpoint
					<input
						required
						type="url"
						value={storage().endpoint}
						onInput={(e) =>
							setStorage({
								...storage(),
								endpoint: e.currentTarget.value,
							})
						}
					/>
				</label>
				<div class="form-row">
					<label>
						Region
						<input
							required
							value={storage().region}
							onInput={(e) =>
								setStorage({
									...storage(),
									region: e.currentTarget.value,
								})
							}
						/>
					</label>
					<label>
						Bucket
						<input
							required
							value={storage().bucket}
							onInput={(e) =>
								setStorage({
									...storage(),
									bucket: e.currentTarget.value,
								})
							}
						/>
					</label>
				</div>
				<label class="wide">
					Access key ID
					<input
						required
						value={storage().accessKeyId}
						onInput={(e) =>
							setStorage({
								...storage(),
								accessKeyId: e.currentTarget.value,
							})
						}
					/>
				</label>
				<label class="wide">
					Secret access key
					<input
						required
						type="password"
						value={storage().secretAccessKey}
						placeholder="Required"
						onInput={(e) =>
							setStorage({
								...storage(),
								secretAccessKey: e.currentTarget.value,
							})
						}
					/>
				</label>
				<label class="check">
					<input
						type="checkbox"
						checked={storage().forcePathStyle}
						onChange={(e) =>
							setStorage({
								...storage(),
								forcePathStyle: e.currentTarget.checked,
							})
						}
					/>
					<span>Use path-style URLs</span>
				</label>
				<div class="actions">
					<button class="primary" type="submit">
						Connect storage
					</button>
					<span class="form-status">{storageStatus()}</span>
				</div>
			</form>
		</section>
	);
}

function RecordingsPanel() {
	const recordingsQuery = createQuery(() => recordingsQueryOpts);
	const [archivingId, setArchivingId] = createSignal<string>();
	const archiveMutation = createMutation(() => ({
		mutationFn: (id: string) =>
			runApi(client.uploads.archive({ params: { id } })),
		onMutate: (id) => setArchivingId(id),
		onSuccess: () => void recordingsQuery.refetch(),
		onSettled: () => setArchivingId(undefined),
	}));

	const recordings = () => recordingsQuery.data ?? [];

	const recordingsStatus = () => {
		if (recordingsQuery.isError)
			return recordingsQuery.error instanceof Error
				? recordingsQuery.error.message
				: "Could not load recordings";
		if (archiveMutation.isError)
			return archiveMutation.error instanceof Error
				? archiveMutation.error.message
				: "Could not archive recording";
		return "";
	};

	return (
		<section class="panel recordings-panel">
			<div class="panel-heading">
				<div>
					<h2>Recordings</h2>
				</div>
				<button
					class="text-button"
					onClick={() => void recordingsQuery.refetch()}
				>
					Refresh
				</button>
			</div>
			<Suspense
				fallback={
					<div class="recording-list">
						<p class="empty">Loading recordings...</p>
					</div>
				}
			>
				<div class="recording-list">
					<For
						each={recordings()}
						fallback={
							<p class="empty">{recordingsStatus() || "No recordings yet."}</p>
						}
					>
						{(recording) => (
							<div class="recording-row">
								<div class="recording-primary">
									<span class={`recording-state ${recording.status}`} />
									<div>
										<strong>{recording.name}</strong>
									</div>
								</div>
								<div class="recording-meta">
									<span>{recording.format.toUpperCase()}</span>
									<span>{recording.status}</span>
									<span>{recording.privacy}</span>
									<Show when={recording.status === "complete"}>
										<a
											href={recording.viewerUrl}
											target="_blank"
											rel="noreferrer"
										>
											View recording
										</a>
									</Show>
									<button
										type="button"
										class="archive-button"
										disabled={archiveMutation.isPending}
										onClick={() => archiveMutation.mutate(recording.id)}
									>
										{archivingId() === recording.id ? "Archiving..." : "Archive"}
									</button>
								</div>
							</div>
						)}
					</For>
				</div>
				<Show when={recordings().length > 0 && recordingsStatus()}>
					<p class="form-status">{recordingsStatus()}</p>
				</Show>
			</Suspense>
		</section>
	);
}

function Sidebar() {
	const storageQuery = createQuery(() => storageQueryOpts);

	return (
		<aside class="sidebar">
			<Show when={storageQuery.data?.data}>
				{(storage) => (
					<section class="panel storage-sidebar-panel">
						<div class="panel-heading">
							<div>
								<h2>Storage</h2>
							</div>
						</div>
						<div class="storage-sidebar-fields">
							<div class="storage-field">
								<span class="storage-label">BUCKET</span>
								<code>{storage().bucket}</code>
							</div>
							<div class="storage-field">
								<span class="storage-label">ENDPOINT</span>
								<code>{storage().endpoint}</code>
							</div>
						</div>
					</section>
				)}
			</Show>
			<CaptureKeysPanel />
		</aside>
	);
}

function CaptureKeysPanel() {
	const [keyName, setKeyName] = createSignal("Blip Capture");
	const [newKey, setNewKey] = createSignal("");

	const keysQuery = createQuery(() => keysQueryOpts);
	const keys = () => keysQuery.data ?? [];

	const createKeyMutation = createMutation(() => ({
		mutationFn: async (name: string) => {
			const { data, error } = await authClient.apiKey.create({ name });
			if (error || !data) {
				throw new Error(error?.message ?? "Could not create key");
			}
			return data.key;
		},
		onSuccess: (key) => {
			setNewKey(key);
			void keysQuery.refetch();
		},
	}));

	const createKey = (event: SubmitEvent) => {
		event.preventDefault();
		createKeyMutation.mutate(keyName());
	};

	const deleteKeyMutation = createMutation(() => ({
		mutationFn: async (id: string) => {
			const { error } = await authClient.apiKey.delete({ keyId: id });
			if (error) {
				throw new Error(error.message ?? "Could not revoke key");
			}
		},
		onSuccess: () => {
			void keysQuery.refetch();
		},
	}));

	const deleteKey = (id: string) => {
		if (
			!confirm("Revoke this key? Blip Capture will stop uploading immediately.")
		)
			return;
		deleteKeyMutation.mutate(id);
	};

	const keyStatus = () => {
		if (createKeyMutation.isPending) return "Creating key...";
		if (createKeyMutation.isError)
			return createKeyMutation.error instanceof Error
				? createKeyMutation.error.message
				: "Could not create key";
		if (deleteKeyMutation.isPending) return "Revoking key...";
		if (deleteKeyMutation.isError)
			return deleteKeyMutation.error instanceof Error
				? deleteKeyMutation.error.message
				: "Could not revoke key";
		if (keysQuery.isError)
			return keysQuery.error instanceof Error
				? keysQuery.error.message
				: "Could not load keys";
		return "";
	};

	return (
		<section class="panel keys-panel">
			<div class="panel-heading">
				<div>
					<h2>Capture keys</h2>
				</div>
			</div>
			<form class="key-form" onSubmit={createKey}>
				<input
					required
					maxlength="32"
					value={keyName()}
					onInput={(e) => setKeyName(e.currentTarget.value)}
					aria-label="Key name"
				/>
				<button class="primary" type="submit">
					Create key
				</button>
			</form>
			<span class="form-status">{keyStatus()}</span>

			<Show when={newKey()}>
				<div class="key-reveal">
					<span>CAPTURE KEY</span>
					<code>{newKey()}</code>
					<div class="key-reveal-actions">
						<button onClick={() => navigator.clipboard.writeText(newKey())}>
							Copy key
						</button>
					</div>
					<small>This secret will not be shown again.</small>
				</div>
			</Show>

			<Suspense
				fallback={
					<div class="key-list">
						<p class="empty">Loading Capture keys...</p>
					</div>
				}
			>
				<div class="key-list">
					<For
						each={keys()}
						fallback={<p class="empty">No Capture keys yet.</p>}
					>
						{(key) => (
							<div class="key-row">
								<div>
									<strong>{String(key.name ?? "Untitled key")}</strong>
									<code>
										{String(key.start ?? key.prefix ?? "blip_")}
										...
									</code>
								</div>
								<div class="key-meta">
									<span>
										{new Date(String(key.createdAt)).toLocaleDateString()}
									</span>
									<button onClick={() => deleteKey(String(key.id))}>
										Revoke
									</button>
								</div>
							</div>
						)}
					</For>
				</div>
			</Suspense>
		</section>
	);
}

function SignIn() {
	return (
		<main class="signin">
			<div class="signin-mark">
				<span class="signal" />
				BLIP / SERVER
			</div>
			<section>
				<p class="eyebrow">RECORD WITHOUT SURRENDER</p>
				<h1>
					Screen recording,
					<br />
					<i>on your terms.</i>
				</h1>
				<p>
					Blip sends recordings directly to storage you own. Sign in to connect
					a bucket and authorize your Capture app.
				</p>
				<button
					class="github"
					onClick={() =>
						authClient.signIn.social({ provider: "github", callbackURL: "/" })
					}
				>
					<span>GH</span> Continue with GitHub
				</button>
			</section>
			<footer>
				<span>SELF-HOSTED</span>
				<span>S3-COMPATIBLE</span>
				<span>END-TO-END CONTROL</span>
			</footer>
		</main>
	);
}

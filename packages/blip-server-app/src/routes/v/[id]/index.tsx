import { Match, Show, Suspense, Switch, createSignal } from "solid-js";
import { useParams } from "@solidjs/router";
import { createMutation, createQuery } from "@tanstack/solid-query";
import { client, runApi } from "../../../ApiClient.ts";
import { authClient } from "../../../auth-client.ts";

export default function Viewer() {
	const params = useParams<{ id: string }>();
	const sessionQuery = createQuery(() => ({
		queryKey: ["session"],
		queryFn: () => authClient.getSession().then((res) => res.data),
		deferStream: true,
	}));
	const session = () => ({
		isPending: sessionQuery.isPending,
		data: sessionQuery.data,
	});
	const viewQuery = createQuery(() => ({
		queryKey: ["view", params.id],
		queryFn: async () => {
			try {
				return await runApi(client.system.view({ params: { id: params.id } }));
			} catch (error) {
				if (
					error &&
					typeof error === "object" &&
					"_tag" in error &&
					error._tag === "PasswordRequired"
				) {
					return {
						id: params.id,
						name: undefined,
						createdAt: undefined,
						privacy: "password",
						owner: false,
						source: undefined,
					};
				}
				throw error;
			}
		},
		deferStream: true,
	}));
	const [password, setPassword] = createSignal("");
	const [privacyInput, setPrivacyInput] = createSignal<string>();
	const [newPassword, setNewPassword] = createSignal("");
	const [nameInput, setNameInput] = createSignal<string>();
	const [editingName, setEditingName] = createSignal(false);
	let nameElement: HTMLHeadingElement | undefined;

	const privacy = () => privacyInput() ?? viewQuery.data?.privacy ?? "public";
	const recordingName = () =>
		nameInput() ??
		viewQuery.data?.name ??
		(viewQuery.data?.createdAt
			? new Date(viewQuery.data.createdAt).toLocaleString()
			: "Recording");

	const mediaSource = () => viewQuery.data?.source;
	const access = () =>
		viewQuery.data?.owner
			? "Owner preview"
			: viewQuery.data?.privacy === "public"
				? "Public recording"
				: "Protected recording";

	const unlockMutation = createMutation(() => ({
		mutationFn: async (pwd: string) => {
			await runApi(
				client.system.unlock({
					params: { id: params.id },
					payload: { password: pwd },
				}),
			);
		},
		onSuccess: () => {
			void viewQuery.refetch();
		},
	}));

	const unlock = (event: SubmitEvent) => {
		event.preventDefault();
		unlockMutation.mutate(password());
	};

	const savePrivacyMutation = createMutation(() => ({
		mutationFn: async (val: {
			privacy: string;
			password?: string | undefined;
		}) => {
			await runApi(
				client.uploads.privacy({
					params: { id: params.id },
					payload: val.password
						? { privacy: val.privacy, password: val.password }
						: { privacy: val.privacy },
				}),
			);
		},
		onSuccess: () => {
			setPrivacyInput(undefined);
			setNewPassword("");
			void viewQuery.refetch();
		},
	}));

	const savePrivacy = () => {
		savePrivacyMutation.mutate({
			privacy: privacy(),
			password: newPassword() || undefined,
		});
	};

	const renameMutation = createMutation(() => ({
		mutationFn: (name: string) =>
			runApi(
				client.uploads.rename({
					params: { id: params.id },
					payload: { name },
				}),
			),
		onSuccess: async () => {
			await viewQuery.refetch();
			setNameInput(undefined);
		},
		onError: () => setNameInput(undefined),
	}));

	const editName = () => {
		if (editingName() || renameMutation.isPending) return;
		setEditingName(true);
		queueMicrotask(() => {
			nameElement?.focus();
			const selection = window.getSelection();
			if (!nameElement || !selection) return;
			const range = document.createRange();
			range.selectNodeContents(nameElement);
			selection.removeAllRanges();
			selection.addRange(range);
		});
	};

	const saveName = () => {
		if (!editingName()) return;
		setEditingName(false);
		const name = nameElement?.textContent?.trim() ?? "";
		if (!name || name === recordingName()) {
			if (nameElement) nameElement.textContent = recordingName();
			return;
		}
		setNameInput(name);
		renameMutation.mutate(name);
	};

	const cancelNameEdit = () => {
		setEditingName(false);
		if (nameElement) nameElement.textContent = recordingName();
		nameElement?.blur();
	};

	return (
		<main class="viewer-shell">
			<header class="viewer-topbar">
				<a class="brand" href="/">
					<span class="signal" />
					BLIP
				</a>
				<div class="viewer-meta">
					<Suspense
						fallback={<span class="viewer-access">Loading recording</span>}
					>
						<span class="viewer-access">{access()}</span>
					</Suspense>
					<Show when={session().data?.user}>
						{(user) => (
							<div class="viewer-user">
								<Show when={user().image}>
									{(url) => <img class="avatar" src={url()} alt="" />}
								</Show>
								<span>{user().name}</span>
							</div>
						)}
					</Show>
				</div>
			</header>

			<div class="viewer-body">
				<section class="viewer-stage">
					<div class="viewer-frame">
						<Switch>
							<Match when={viewQuery.isError}>
								<div class="viewer-gate">
									<span class="eyebrow">RECORDING ACCESS</span>
									<h1>Recording unavailable</h1>
									<p>
										{viewQuery.error instanceof Error
											? viewQuery.error.message
											: "Could not load recording."}
									</p>
								</div>
							</Match>
							<Match when={mediaSource()}>
								{(url) => <video controls autoplay playsinline src={url()} />}
							</Match>
							<Match when={viewQuery.data?.privacy === "password"}>
								<div class="viewer-gate">
									<span class="eyebrow">RECORDING ACCESS</span>
									<h1>Private recording</h1>
									<p>
										Enter the password shared by the owner to watch this
										recording.
									</p>
									<form onSubmit={unlock}>
										<input
											type="password"
											autocomplete="current-password"
											placeholder="Password"
											required
											value={password()}
											onInput={(event) =>
												setPassword(event.currentTarget.value)
											}
										/>
										<button disabled={unlockMutation.isPending}>Watch</button>
									</form>
									<span class="viewer-status">
										{unlockMutation.isPending
											? "Checking..."
											: unlockMutation.isError
												? unlockMutation.error &&
													typeof unlockMutation.error === "object" &&
													"_tag" in unlockMutation.error &&
													unlockMutation.error._tag === "Unauthorized"
													? "That password is not correct."
													: "Could not open recording."
												: ""}
									</span>
								</div>
							</Match>
							<Match when={true}>
								<div class="viewer-gate">
									<span class="eyebrow">RECORDING ACCESS</span>
									<h1>Recording unavailable</h1>
									<p>The owner has made this recording private.</p>
								</div>
							</Match>
						</Switch>
					</div>
				</section>

				<Suspense>
					<Show when={viewQuery.data?.owner}>
						<aside class="viewer-sidebar">
							<h1
								ref={nameElement}
								class="viewer-title"
								classList={{ editing: editingName() }}
								contenteditable={editingName()}
								aria-label="Recording name"
								aria-multiline="false"
								onClick={editName}
								onBlur={saveName}
								onKeyDown={(event) => {
									if (event.key === "Enter") {
										event.preventDefault();
										saveName();
										event.currentTarget.blur();
									} else if (event.key === "Escape") {
										event.preventDefault();
										cancelNameEdit();
									}
								}}
							>
								{recordingName()}
							</h1>
							<div class="viewer-owner">
								<label for="viewer-privacy">Who can view</label>
								<select
									id="viewer-privacy"
									value={privacy()}
									onChange={(event) =>
										setPrivacyInput(event.currentTarget.value)
									}
								>
									<option value="public">Anyone with the link</option>
									<option value="password">Anyone with a password</option>
									<option value="private">Only me</option>
								</select>
								<Show when={privacy() === "password"}>
									<input
										type="password"
										placeholder="New password"
										aria-label="New password"
										value={newPassword()}
										onInput={(event) =>
											setNewPassword(event.currentTarget.value)
										}
									/>
								</Show>
								<button
									class="primary"
									onClick={savePrivacy}
									disabled={savePrivacyMutation.isPending}
								>
									Save
								</button>
								<span class="viewer-status">
									{savePrivacyMutation.isPending
										? "Saving..."
										: savePrivacyMutation.isError
											? savePrivacyMutation.error instanceof Error
												? savePrivacyMutation.error.message
												: "Could not save"
											: savePrivacyMutation.isSuccess
												? "Saved"
												: ""}
								</span>
							</div>
						</aside>
					</Show>
				</Suspense>
			</div>
		</main>
	);
}

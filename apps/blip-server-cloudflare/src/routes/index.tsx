import { For, Show, Suspense, createEffect, createSignal } from "solid-js"
import { createMutation, createQuery } from "@tanstack/solid-query"
import { client, runApi } from "../api-client.ts"
import { authClient } from "../auth-client.ts"

const emptyStorage = {
  endpoint: "https://s3.amazonaws.com",
  region: "us-east-1",
  bucket: "",
  accessKeyId: "",
  secretAccessKey: "",
  forcePathStyle: false
}

export default function Dashboard() {
  const sessionQuery = createQuery(() => ({
    queryKey: ["session"],
    queryFn: () => authClient.getSession().then((res) => res.data),
    deferStream: true
  }))
  const session = () => ({
    isPending: sessionQuery.isPending,
    data: sessionQuery.data
  })
  const [storage, setStorage] = createSignal({ ...emptyStorage })
  const [keyName, setKeyName] = createSignal("Blip Capture")
  const [newKey, setNewKey] = createSignal("")
  const storageQuery = createQuery(() => ({
    queryKey: ["storage"],
    queryFn: async () => {
      try {
        const value = await runApi(client.storage.get())
        return { exists: true, data: value }
      } catch (error) {
        if (error && typeof error === "object" && "_tag" in error && error._tag === "NotFound") {
          return { exists: false, data: undefined }
        }
        throw error
      }
    },
    enabled: !session().isPending && !!session().data?.user,
    deferStream: true
  }))

  createEffect(() => {
    const value = storageQuery.data?.data
    if (value) {
      setStorage((current) => ({ ...current, ...value, secretAccessKey: "" }))
    }
  })

  const storageExists = () => !!storageQuery.data?.exists

  const keysQuery = createQuery(() => ({
    queryKey: ["keys"],
    queryFn: async () => {
      const { data, error } = await authClient.apiKey.list({ query: { limit: 100 } })
      if (error) throw new Error(error.message)
      return (data?.apiKeys ?? []) as Array<Record<string, unknown>>
    },
    enabled: !session().isPending && !!session().data?.user,
    deferStream: true
  }))

  const keys = () => keysQuery.data ?? []

  const recordingsQuery = createQuery(() => ({
    queryKey: ["recordings"],
    queryFn: () => runApi(client.uploads.list()),
    enabled: !session().isPending && !!session().data?.user,
    deferStream: true
  }))

  const recordings = () => recordingsQuery.data ?? []
  const refreshRecordings = () => void recordingsQuery.refetch()

  const saveStorageMutation = createMutation(() => ({
    mutationFn: async (val: typeof emptyStorage) => {
      await runApi(client.storage.set({
        payload: val
      }))
    },
    onSuccess: () => {
      setStorage((current) => ({ ...current, secretAccessKey: "" }))
      void storageQuery.refetch()
    }
  }))

  const saveStorage = (event: SubmitEvent) => {
    event.preventDefault()
    saveStorageMutation.mutate(storage())
  }

  const removeStorageMutation = createMutation(() => ({
    mutationFn: async () => {
      await runApi(client.storage.remove())
    },
    onSuccess: () => {
      setStorage({ ...emptyStorage })
      void storageQuery.refetch()
    }
  }))

  const removeStorage = () => {
    if (!confirm("Disconnect storage? Existing recording links will continue to work.")) return
    removeStorageMutation.mutate()
  }

  const storageStatus = () => {
    if (saveStorageMutation.isPending) return "Testing connection..."
    if (saveStorageMutation.isError) return saveStorageMutation.error instanceof Error ? saveStorageMutation.error.message : "Could not connect storage"
    if (saveStorageMutation.isSuccess) return "Storage connected"
    if (removeStorageMutation.isPending) return "Disconnecting..."
    if (removeStorageMutation.isError) return removeStorageMutation.error instanceof Error ? removeStorageMutation.error.message : "Could not disconnect storage"
    if (removeStorageMutation.isSuccess) return "Storage disconnected"
    if (storageQuery.isError) return storageQuery.error instanceof Error ? storageQuery.error.message : "Could not load storage"
    return ""
  }

  const createKeyMutation = createMutation(() => ({
    mutationFn: async (name: string) => {
      const { data, error } = await authClient.apiKey.create({ name })
      if (error || !data) {
        throw new Error(error?.message ?? "Could not create key")
      }
      return data.key
    },
    onSuccess: (key) => {
      setNewKey(key)
      void keysQuery.refetch()
    }
  }))

  const createKey = (event: SubmitEvent) => {
    event.preventDefault()
    createKeyMutation.mutate(keyName())
  }

  const deleteKeyMutation = createMutation(() => ({
    mutationFn: async (id: string) => {
      const { error } = await authClient.apiKey.delete({ keyId: id })
      if (error) {
        throw new Error(error.message ?? "Could not revoke key")
      }
    },
    onSuccess: () => {
      void keysQuery.refetch()
    }
  }))

  const deleteKey = (id: string) => {
    if (!confirm("Revoke this key? Blip Capture will stop uploading immediately.")) return
    deleteKeyMutation.mutate(id)
  }

  const keyStatus = () => {
    if (createKeyMutation.isPending) return "Creating key..."
    if (createKeyMutation.isError) return createKeyMutation.error instanceof Error ? createKeyMutation.error.message : "Could not create key"
    if (deleteKeyMutation.isPending) return "Revoking key..."
    if (deleteKeyMutation.isError) return deleteKeyMutation.error instanceof Error ? deleteKeyMutation.error.message : "Could not revoke key"
    if (keysQuery.isError) return keysQuery.error instanceof Error ? keysQuery.error.message : "Could not load keys"
    return ""
  }

  const recordingsStatus = () => {
    if (recordingsQuery.isPending) return "Loading recordings..."
    if (recordingsQuery.isError) return recordingsQuery.error instanceof Error ? recordingsQuery.error.message : "Could not load recordings"
    return ""
  }

  const captureUrl = () => `${window.location.origin}#${newKey()}`
  const captureAppUrl = () => `blip-capture://add-profile?url=${encodeURIComponent(captureUrl())}`

  return <Show when={!session().isPending} fallback={<div class="boot">BLIP / CONNECTING</div>}>
    <Show when={session().data?.user} fallback={<SignIn />}>
      {(user) => <main class="shell">
        <header class="topbar">
          <a class="brand" href="/"><span class="signal" />BLIP</a>
          <div class="identity">
            <span>{user().name}</span>
            <button class="text-button" onClick={async () => { await authClient.signOut(); void sessionQuery.refetch() }}>Sign out</button>
          </div>
        </header>

        <section class="hero">
          <p class="eyebrow">PRIVATE RECORDING INFRASTRUCTURE</p>
          <h1>Your screen.<br /><i>Your storage.</i></h1>
          <p class="lede">Connect an S3-compatible bucket, create a Capture key, and keep every recording under your control.</p>
          <Suspense fallback={<div class="status-line"><span class="idle" />Loading storage</div>}>
            <div class="status-line"><span class={storageExists() ? "live" : "idle"} />{storageExists() ? "Storage online" : "Setup incomplete"}</div>
          </Suspense>
        </section>

        <div class="grid">
          <Suspense fallback={<section class="panel storage-panel"><p class="empty">Loading storage...</p></section>}>
            <section class="panel storage-panel">
              <div class="panel-heading">
                <div><span class="index">01</span><h2>Storage</h2></div>
                <span class="panel-note">S3 / R2 / MinIO</span>
              </div>
              <form onSubmit={saveStorage}>
                <label class="wide">Endpoint<input required type="url" value={storage().endpoint} onInput={(e) => setStorage({ ...storage(), endpoint: e.currentTarget.value })} /></label>
                <div class="form-row">
                  <label>Region<input required value={storage().region} onInput={(e) => setStorage({ ...storage(), region: e.currentTarget.value })} /></label>
                  <label>Bucket<input required value={storage().bucket} onInput={(e) => setStorage({ ...storage(), bucket: e.currentTarget.value })} /></label>
                </div>
                <label class="wide">Access key ID<input required value={storage().accessKeyId} onInput={(e) => setStorage({ ...storage(), accessKeyId: e.currentTarget.value })} /></label>
                <label class="wide">Secret access key<input required type="password" value={storage().secretAccessKey} placeholder={storageExists() ? "Enter to update credentials" : "Required"} onInput={(e) => setStorage({ ...storage(), secretAccessKey: e.currentTarget.value })} /></label>
                <label class="check"><input type="checkbox" checked={storage().forcePathStyle} onChange={(e) => setStorage({ ...storage(), forcePathStyle: e.currentTarget.checked })} /><span>Use path-style URLs</span></label>
                <div class="actions">
                  <button class="primary" type="submit">{storageExists() ? "Update connection" : "Connect storage"}</button>
                  <Show when={storageExists()}><button class="danger" type="button" onClick={removeStorage}>Disconnect</button></Show>
                  <span class="form-status">{storageStatus()}</span>
                </div>
              </form>
            </section>
          </Suspense>

          <section class="panel keys-panel">
            <div class="panel-heading">
              <div><span class="index">02</span><h2>Capture keys</h2></div>
              <span class="panel-note">BEARER ACCESS</span>
            </div>
            <p class="panel-copy">Keys connect Blip Capture to this server. Each key is shown once and can be revoked independently.</p>
            <form class="key-form" onSubmit={createKey}>
              <input required maxlength="32" value={keyName()} onInput={(e) => setKeyName(e.currentTarget.value)} aria-label="Key name" />
              <button class="primary" type="submit">Create key</button>
            </form>
            <span class="form-status">{keyStatus()}</span>

            <Show when={newKey()}>
              <div class="key-reveal">
                <span>ADD THIS URL TO BLIP CAPTURE</span>
                <code>{captureUrl()}</code>
                <div class="key-reveal-actions">
                  <a href={captureAppUrl()}>Add to Blip Capture</a>
                  <button onClick={() => navigator.clipboard.writeText(captureUrl())}>Copy URL</button>
                </div>
                <small>This secret will not be shown again.</small>
              </div>
            </Show>

            <Suspense fallback={<div class="key-list"><p class="empty">Loading Capture keys...</p></div>}>
              <div class="key-list">
                <For each={keys()} fallback={<p class="empty">No Capture keys yet.</p>}>
                  {(key) => <div class="key-row">
                    <div><strong>{String(key.name ?? "Untitled key")}</strong><code>{String(key.start ?? key.prefix ?? "blip_")}...</code></div>
                    <div class="key-meta"><span>{new Date(String(key.createdAt)).toLocaleDateString()}</span><button onClick={() => deleteKey(String(key.id))}>Revoke</button></div>
                  </div>}
                </For>
              </div>
            </Suspense>
          </section>
        </div>

        <section class="panel recordings-panel">
          <div class="panel-heading">
            <div><span class="index">03</span><h2>Recordings</h2></div>
            <button class="text-button" onClick={refreshRecordings}>Refresh</button>
          </div>
          <Suspense fallback={<div class="recording-list"><p class="empty">Loading recordings...</p></div>}>
            <div class="recording-list">
              <For each={recordings()} fallback={<p class="empty">{recordingsStatus() || "No recordings yet."}</p>}>
                {(recording) => <div class="recording-row">
                  <div class="recording-primary">
                    <span class={`recording-state ${recording.status}`} />
                    <div>
                      <strong>{new Date(recording.createdAt).toLocaleString()}</strong>
                      <code>{recording.id}</code>
                    </div>
                  </div>
                  <div class="recording-meta">
                    <span>{recording.format.toUpperCase()}</span>
                    <span>{recording.status}</span>
                    <span>{recording.privacy}</span>
                    <Show when={recording.status === "complete"}>
                      <a href={recording.viewerUrl} target="_blank" rel="noreferrer">View recording</a>
                    </Show>
                  </div>
                </div>}
              </For>
            </div>
            <Show when={recordings().length > 0 && recordingsStatus()}>
              <p class="form-status">{recordingsStatus()}</p>
            </Show>
          </Suspense>
        </section>
      </main>}
    </Show>
  </Show>
}

function SignIn() {
  return <main class="signin">
    <div class="signin-mark"><span class="signal" />BLIP / SERVER</div>
    <section>
      <p class="eyebrow">RECORD WITHOUT SURRENDER</p>
      <h1>Screen recording,<br /><i>on your terms.</i></h1>
      <p>Blip sends recordings directly to storage you own. Sign in to connect a bucket and authorize your Capture app.</p>
      <button class="github" onClick={() => authClient.signIn.social({ provider: "github", callbackURL: "/" })}>
        <span>GH</span> Continue with GitHub
      </button>
    </section>
    <footer><span>SELF-HOSTED</span><span>S3-COMPATIBLE</span><span>END-TO-END CONTROL</span></footer>
  </main>
}

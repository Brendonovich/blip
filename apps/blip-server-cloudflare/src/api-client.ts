import { BlipApi } from "@blip/server-core"
import { QueryClient } from "@tanstack/solid-query"
import * as Effect from "effect/Effect"
import * as FetchHttpClient from "effect/unstable/http/FetchHttpClient"
import * as HttpClient from "effect/unstable/http/HttpClient"
import * as HttpClientRequest from "effect/unstable/http/HttpClientRequest"
import * as HttpApiClient from "effect/unstable/httpapi/HttpApiClient"
import { getRequestEvent, isServer } from "solid-js/web"
import { getInMemoryHttpClient } from "./in-memory-api.ts"

const httpClient = HttpClient.make((request) => {
  if (isServer) {
    const memoryClient = getInMemoryHttpClient()
    if (memoryClient) {
      const headers = getRequestEvent()?.request.headers
      const forwarded = headers
        ? HttpClientRequest.setHeaders(request, ["authorization", "cookie"].flatMap((name) => {
          const value = headers.get(name)
          return value ? [[name, value] as const] : []
        }))
        : request
      return memoryClient.execute(forwarded)
    }
  }
  return HttpClient.execute(request).pipe(Effect.provide(FetchHttpClient.layer))
})

export const client = Effect.runSync(HttpApiClient.makeWith(BlipApi, { httpClient }))

export const runApi = <A, E>(effect: Effect.Effect<A, E, never>): Promise<A> =>
  Effect.runPromise(effect)

export const makeQueryClient = () => new QueryClient()

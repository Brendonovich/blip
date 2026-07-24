import type * as HttpClient from "effect/unstable/http/HttpClient"

export type AuthHandler = (request: Request) => Promise<Response>

let inMemoryClient: HttpClient.HttpClient | undefined
let inMemoryAuthHandler: AuthHandler | undefined

export const setInMemoryHttpClient = (client: HttpClient.HttpClient) => {
  inMemoryClient = client
}

export const getInMemoryHttpClient = () => inMemoryClient

export const setInMemoryAuthHandler = (handler: AuthHandler) => {
  inMemoryAuthHandler = handler
}

export const getInMemoryAuthHandler = () => inMemoryAuthHandler

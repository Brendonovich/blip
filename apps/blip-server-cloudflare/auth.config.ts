import { makeAuth } from "./src/auth.ts"

const statement = {
  bind() { return this },
  first: async () => null,
  run: async () => ({ success: true, results: [], meta: {} }),
  all: async () => ({ success: true, results: [], meta: {} }),
  raw: async () => []
}

const database = {
  prepare: () => statement,
  batch: async () => [],
  exec: async () => ({ count: 0, duration: 0 }),
  dump: async () => new ArrayBuffer(0)
} as unknown as D1Database

export const auth = makeAuth(database, {
  baseUrl: "http://localhost:3000",
  secret: "schema-generation-secret-at-least-32-characters",
  githubClientId: "schema-generation",
  githubClientSecret: "schema-generation"
})

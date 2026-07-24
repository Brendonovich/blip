import { Router } from "@solidjs/router"
import { FileRoutes } from "@solidjs/start/router"
import { QueryClientProvider } from "@tanstack/solid-query"
import { Suspense } from "solid-js"
import { makeQueryClient } from "./api-client.ts"
import "./styles.css"

export default function App() {
  const queryClient = makeQueryClient()

  return (
    <QueryClientProvider client={queryClient}>
      <Router root={(props) => <Suspense>{props.children}</Suspense>}>
        <FileRoutes />
      </Router>
    </QueryClientProvider>
  )
}

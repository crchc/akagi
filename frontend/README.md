# Akagi Web UI

React 19 + TypeScript + Vite frontend served by Akagi's loopback HTTP server.

- `src/lib/api.ts` provides JSON command calls and the shared SSE connection.
- `src/hooks/useBackendBridge.ts` hydrates Zustand stores and handles events.
- `src/routes/Suggestions.tsx` is the dedicated live suggestion tab.
- Vite proxies `/api` to `127.0.0.1:3000` during development.

```bash
npm ci
npm run dev      # development UI on :1420
npm run build    # production assets in dist/
npm test
npm run lint
```

Run the Rust backend separately with `cargo run -- --no-open` when using the
Vite development server.

# Frontend/backend contract

The browser calls `POST /api/invoke/<command>` with a JSON object. Responses
are `{ "ok": true, "value": ... }` or an error envelope. Live data uses one
`GET /api/events` Server-Sent Events connection. The implementation and full
command dispatch table live in `src/web.rs`; shared payload types live in
`src/schema/` and are mirrored by `frontend/src/types.ts`.

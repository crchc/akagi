# Local Web integration

`state.rs` owns shared application handles, `commands.rs` implements UI
operations, and `capture_supervisor.rs` controls capture startup and shutdown.
`src/web.rs` maps these operations onto loopback HTTP routes and streams bus
events to browsers over SSE.

The server binds only to `127.0.0.1`. Commands use
`POST /api/invoke/<name>` with JSON; live events share one
`GET /api/events` connection.

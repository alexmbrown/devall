# JSONPlaceholder demo project

A small /dev/all project you can open in the app (Projects → Open project) or run from a terminal:

```bash
devall run examples/jsonplaceholder --env public
```

`posts/` inherits a bearer token from `{{secret.API_TOKEN}}`. The value is never stored here:
set it in the app (Environments → Public API → Secrets) or, in CI, via `DEVALL_SECRET_API_TOKEN`.
JSONPlaceholder ignores the header, so any value works.

Also included:

- `mocks/mappings/list-users.json` — a mock of `GET /users` that returns randomized users
  (`devall mock serve --project examples/jsonplaceholder`, or Mock server → Start in the app).
- `load/users-smoke.load.yaml` — a 5-second load test with `target: mock`, so it runs against the
  project's own mock server and never loads the public API:
  `devall load run users-smoke.load.yaml --project examples/jsonplaceholder`.
- `http/list-users.req.yaml` has a pre-request and a post-response script (`pm.*` API).

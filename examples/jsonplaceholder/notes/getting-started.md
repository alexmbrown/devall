---
tags: [onboarding]
---
# Getting started

This project talks to [JSONPlaceholder](https://jsonplaceholder.typicode.com), a free fake REST API.

- [List users](http/list-users.req.yaml) captures `{{firstUserId}}` and checks the response shape.
- Everything under [Posts](http/posts/get-post.req.yaml) inherits a bearer token from `{{secret.API_TOKEN}}` —
  set it under **Environments → Public API → Secrets** (any value works here).

Run everything from a terminal or CI:

```bash
DEVALL_SECRET_API_TOKEN=demo devall run examples/jsonplaceholder --env public --junit report.xml
```

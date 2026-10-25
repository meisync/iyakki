# __TITLE__

A web service for the person's k3s cluster (IYAKKI kind: service). It listens on `0.0.0.0:$PORT` (default 8080), answers
`GET /healthz`, logs to stdout and stops on SIGTERM.

* Test (no cluster): `python3 tests/test_server.py`
* Build and test in a container, deploy and verify: the IDE's deploy tools, or the checklist in `docs/development/CLAUDE-CODE-APP-DEVELOPMENT.md`, section 5.
* `deploy/app.yaml` is a Deployment and a Service. It has no Namespace: the workspace namespace is used. Pin the image tag to the one the build returns.

---
name: k3s-web-service-patterns
description: Design rules for any web service or API on k3s, in any language: the container contract, config and secrets, state and volumes, Services, probes, rollouts, tests and a test page
triggers: web service, rest api, microservice, stateless, healthz, persistent volume, configmap, service port, test page, rollout
---
Use to design or review a web service for k3s in any language (Node: k3s-node-webapp; others: the lang-* skill). Build, ship, verify: k3s-build-and-deploy. ACT yourself.

CONTRACT every service keeps:
1. Listen on 0.0.0.0 at $PORT (default 8080). GET /healthz answers 200 as soon as the process can serve, GET /readyz only when its dependencies answer.
2. Logs to stdout, one line per event, never secrets. Exit 0 on SIGTERM after finishing open requests (the pod gets 30 s).
3. Config from environment variables with defaults; secrets via vault_request, a Secret and env valueFrom; never in the image, a manifest or a log.
4. Stateless by default: the container filesystem is thrown away on every redeploy. Anything that must stay goes to a volume, a database or the cluster registry, never to the image.

STATE
5. A volume: PersistentVolumeClaim ReadWriteOnce, storage class local-path (k3s default), mounted at /data. With a ReadWriteOnce volume use replicas 1 and strategy Recreate, otherwise the new pod waits for a volume the old pod still holds. Several replicas need a shared store, not a file.
6. Migrations run at start (idempotent) or as a Job before the rollout, never by hand. Keep the old and new version able to run on the same data for one release.

NETWORK
7. Service ClusterIP when only other pods call it (name.namespace.svc is the address), NodePort when the person opens it. Port, containerPort, targetPort and $PORT are the same number. No Ingress unless the person asks.
8. Browsers allow camera, microphone, clipboard and service workers only on https or on 127.0.0.1. A plain http NodePort cannot use them: say so before building, and offer a forward to 127.0.0.1 on the person's computer.

RUNNING WELL
9. requests cpu 50m memory 64Mi, limits cpu 500m memory 256Mi as a start; raise one step after OOMKilled or throttling. runAsNonRoot with a numeric user, allowPrivilegeEscalation false, no privileged, hostPath or hostNetwork.
10. Readiness and liveness probes on /healthz (liveness never checks dependencies). Expose /metrics (Prometheus text) when graphs are wanted.
11. Version the API (/v1/...), validate input, time out every outgoing call, cap request size, return JSON errors with a stable code, never a stack trace.

TESTS (a service is not done until all three pass)
12. Unit tests in the Dockerfile test stage (deploy_build target=test). Integration tests as their own image run with deploy_run_job (BASE_URL = the Service DNS name). A smoke request to the "reachable at" address after the rollout: /healthz and one real page.
13. For a hardware or browser feature (microphone, camera, audio) also write a tiny static test page: one HTML file, buttons, a visible log of each step and error, no third-party scripts.

DEPLOY AND CLEAN UP
14. New tag per build (never latest), keep the previous tag to roll back, one change at a time. Delete only what you created, never a namespace or kind=all, and only when told.
15. NO IMAGE BUILDER: when deploy_build says neither podman nor docker is installed, stop. Never apply an image that was never built (it ends in ImagePullBackOff). Say so, then for a small app (a few files, under 1 MB) run a stock image (node:22-alpine, python:3.12-slim, nginx:1.27-alpine, pinned tag) with the files in a ConfigMap mounted read-only at /app and the command that starts them; the linter only warns about a docker.io image. A bigger app needs a builder: tell the person to install podman.

Board: track steps with board_add (done only after a passed check). Bugs and failures: own board row with a B- id (never F-); bug_report captures logs; fix, verify, attach the passing run.

Example
Person: "a small API that stores notes, keep them after restarts"
You: Deployment replicas 1 strategy Recreate, a 1Gi local-path claim at /data, /healthz and /v1/notes, tests in the test stage; then deploy and report the address and that a restart kept the notes.

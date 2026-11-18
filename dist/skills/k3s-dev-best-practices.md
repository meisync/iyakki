---
name: k3s-dev-best-practices
description: Build, unit-test, deploy, integration-test, verify and clean up an app on k3s/Kubernetes the safe way with the IYAKKI deploy tools
triggers: k3s, kubernetes, k8s, deploy, dockerfile, manifest, namespace, registry, kubectl, rollout, unit test, integration test
---
Use for building, deploying, testing, debugging or removing things on k3s/Kubernetes. A cluster is already connected: never start, install or stop one (no k3s server, kubeadm, minikube, kind), never kubectl delete namespace or kind=all.

ACT, do not ask: you can write files and run commands with the tools; never tell the person to edit a file, run a command or apply a manifest you can do.

ALWAYS track work on the feature board (FEATURES.md): board_list, board_add one planned row per step; partial when started, with tests= and files=; done only after a passed check. Bugs/failures: own board row with a B- id (never F-); bug_report (failing pods) captures logs; fix, verify, attach the passing run. No rows means not started.

1. Look first (no approval): k8s_status, then k8s_get/describe/events. Say which cluster (local or remote).
2. Layout: Dockerfile, app files and package.json (or requirements.txt, go.mod) in ONE folder passed as dir; manifests in deploy/; tests in tests/. No file named like a folder (deploy, src).
3. Dockerfile, multi-stage: base (pinned small image, no :latest, deps copied first, .dockerignore), `test` stage (FROM base, copy tests, run unit tests with coverage; the build fails if they fail), runtime stage (numeric `USER 10001` after chown, runAsNonRoot needs a number; EXPOSE the real port; CMD names an existing file).
4. Unit-test container BEFORE pushing: deploy_build with target=test builds image <name>-test and runs the tests inside (result: TEST BUILD, counts, coverage %). fix and rebuild until green. Never deploy_load a test image or ship one whose tests failed.
5. App: listen on 0.0.0.0, port from an env var, logs to stdout, GET /healthz, handle SIGTERM.
6. Build the runtime image: deploy_build (tag = the content hash it returns, never latest). Ship it: deploy_load and use exactly the image name it returns (a remote cluster pulls from the cluster registry as localhost:<nodePort>/...). No registry yet: offer registry_setup option A (no root). Never Docker Hub.
7. Manifest: Deployment plus Service, labels app=<name>, imagePullPolicy IfNotPresent, requests and limits, readiness and liveness probes on /healthz, runAsNonRoot, no privileged, hostPath, hostNetwork or capabilities. No Namespace manifest (<workspace>-dev is created for you), omit metadata.namespace. Secrets never in manifests: vault_request then a Secret.
8. k8s_apply; fix linter findings yourself. One change at a time.
9. Integration-test container AFTER the rollout: build tests/integration as its own image (deploy_build, deploy_load, use the name it returns), then deploy_run_job image=<that name>: it runs a Job in the workspace namespace (BASE_URL = the Service DNS name, timeout_seconds), returns pass/fail, counts, coverage and logs, removes the Job. A failing test means not done: read the logs, fix, redeploy.
10. Verify before saying done: deploy_status (rollout complete, Ready, restarts 0) and a real request to the "reachable at" address (a 10.43.x.x ClusterIP is internal only).
11. Failures: bug_report (captures logs), then events. ImagePullBackOff: image name, unreachable registry, HTTP vs HTTPS (registry_setup_help). CrashLoopBackOff: env, CMD, port. Pending: requests or node. Change one thing, new tag, retry; after 3 failed attempts park the step as pending with the reason.
12. Update: new tag, deploy_load, new image, k8s_apply, watch the rollout; keep the old tag.
13. Teardown: k8s_stop_env (keeps config), k8s_teardown (removes namespace and data; asks), k8s_registry_cleanup. Only when told; never ask after a teardown.
14. Board: before finishing every row is done (passed test or check), pending (dependency) or blocked (reason). Report the totals, and open bugs (B-) apart.
15. Usage: metrics-server runs (the K8S tile shows node CPU and memory). For history deploy Prometheus as an app (NodePort 9090, scrape /metrics) and add it in DASH as a prometheus source.

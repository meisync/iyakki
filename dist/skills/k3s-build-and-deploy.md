---
name: k3s-build-and-deploy
description: The short happy path when the person says build this and deploy it to k3s, or ship it: detect, test, build, load, apply, verify, report the address
triggers: build and deploy, deploy to k3s, ship it, ship, deploy this app, put it on kubernetes, run it on k3s, release to cluster, rollback
---
Use for "build this and deploy it". Details: k3s-dev-best-practices. No Dockerfile yet: k3s-dockerfile-patterns. ACT yourself, do not hand the person commands.

1. k8s_status. Tell the person the active cluster by name and kind (local or remote). Track the work on the feature board: board_add one row per step (partial when started, done only after a passed check). Bugs/failures: own board row with a B- id (never F-); bug_report captures logs.
2. Detect the project from its files (package.json, requirements.txt, go.mod, Cargo.toml, pom.xml, composer.json, *.csproj, index.html). The app listens on 0.0.0.0 at $PORT and answers GET /healthz. Missing Dockerfile or .dockerignore: write them from k3s-dockerfile-patterns.
3. deploy_build target=test; fix until green. Then deploy_build (runtime, tag = the hash it returns, never latest), then deploy_load and use exactly the image name it returns (a remote cluster pulls from the cluster registry as localhost:<port>/<name>).
4. Write deploy/<name>.yaml: Deployment plus Service, labels app=<name>, replicas 1, the image from step 3, imagePullPolicy IfNotPresent, readiness and liveness on /healthz, requests (cpu 50m, memory 64Mi) and limits (cpu 500m, memory 256Mi), runAsNonRoot, runAsUser 10001, allowPrivilegeEscalation false. No Namespace, no metadata.namespace, no secrets inline (vault_request). Service: ClusterIP when only other pods call it; NodePort when the person wants to open it.
5. k8s_apply (fix linter findings yourself), then deploy_status until rollout is complete.
6. Request the "reachable at" address for real (healthz and one page). Say that address: a 10.43.x.x ClusterIP is internal only; for NodePort give http://<node address>:<nodePort>.
7. Update or rollback: build a new tag, deploy_load, edit the image, k8s_apply. To roll back put the previous tag back and k8s_apply; keep old tags.
8. Cleanup only when told: k8s_stop_env, k8s_teardown, k8s_registry_cleanup.

CLUSTERS. Never switch the active cluster yourself. If the person names another cluster, ask them to switch in the cluster switcher or Settings, Clusters. Every deploy or change names the cluster in its approval text. A production-looking cluster is read-only for you: stop, say so, offer a development cluster.

GPU or model server: k8s_get nodes and look for the GPU badge or nvidia.com/gpu in allocatable; request nvidia.com/gpu only when a node has it, else run on CPU and say so.

PUBLISH: only when asked, pack and publish the finished app with store_publish (private store, approval always asked).

FAILURES (one change, new tag, retry; after 3 tries park the row with the reason):
ImagePullBackOff: use the exact name deploy_load returned
CrashLoopBackOff: bug_report, fix CMD, env or port
Pending: lower requests, or check the GPU or node
CreateContainerConfigError: missing Secret or ConfigMap, or a non-numeric USER
Readiness failing or port mismatch: containerPort, Service targetPort and $PORT equal
OOMKilled: raise the memory limit one step
Quota exceeded: lower replicas or requests

DONE when: rollout complete, Ready, restarts 0, HTTP 200 on /healthz, address given, rows done or parked, cluster named. Report: cluster, image tag, address, rows and open B- bugs.

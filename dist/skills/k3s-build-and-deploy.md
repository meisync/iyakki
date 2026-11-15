---
name: k3s-build-and-deploy
description: The short happy path when the person says build this and deploy it to k3s, or ship it: detect, test, build, load, apply, verify, report the address
triggers: build and deploy, deploy to k3s, ship it, ship, deploy this app, put it on kubernetes, run it on k3s, release to cluster, rollback
---
Use for "build this and deploy it". Details: k3s-dev-best-practices. No Dockerfile yet: k3s-dockerfile-patterns. ACT yourself, do not hand the person commands.

1. k8s_status. Tell the person the active cluster by name and kind (local or remote). Track the work on the board: board_add one row per step (done only after a passed check). A bug or failure gets its own row with a B- id (never F-): call bug_report to capture the logs.
2. Detect the project from its files (package.json, requirements.txt, go.mod, Cargo.toml, pom.xml, *.csproj, index.html). The app listens on 0.0.0.0 at $PORT and answers GET /healthz. Missing Dockerfile or .dockerignore: write them from k3s-dockerfile-patterns.
3. deploy_build target=test; fix until green. Then deploy_build (runtime, tag = the hash it returns, never latest), then deploy_load and use exactly the image name it returns (a remote cluster pulls from the cluster registry as localhost:<port>/<name>).
4. Write deploy/<name>.yaml by COPYING the template below and changing only NAME, IMAGE (the name from step 3) and PORT; keep every other line (probes, requests and limits, runAsNonRoot are required). No Namespace, no metadata.namespace, no secrets inline (vault_request). Service type ClusterIP when only other pods call it, NodePort when the person wants to open it.
```
apiVersion: apps/v1
kind: Deployment
metadata: {name: NAME, labels: {app: NAME}}
spec:
  replicas: 1
  selector: {matchLabels: {app: NAME}}
  template:
    metadata: {labels: {app: NAME}}
    spec:
      securityContext: {runAsNonRoot: true, runAsUser: 10001}
      containers:
      - name: NAME
        image: IMAGE
        imagePullPolicy: IfNotPresent
        ports: [{containerPort: PORT}]
        readinessProbe: {httpGet: {path: /healthz, port: PORT}}
        livenessProbe: {httpGet: {path: /healthz, port: PORT}}
        resources: {requests: {cpu: 50m, memory: 64Mi}, limits: {cpu: 500m, memory: 256Mi}}
        securityContext: {allowPrivilegeEscalation: false}
---
apiVersion: v1
kind: Service
metadata: {name: NAME}
spec:
  type: NodePort
  selector: {app: NAME}
  ports: [{port: PORT, targetPort: PORT}]
```
5. k8s_apply (fix linter findings yourself), then deploy_status until rollout is complete.
6. Request the "reachable at" address for real (healthz and one page) and say it: a 10.43.x.x ClusterIP is internal only; NodePort is http://<node address>:<nodePort>.
7. Update: new tag, deploy_load, edit the image, k8s_apply. Roll back: put the previous tag back, k8s_apply.
8. Cleanup only when told: k8s_stop_env, k8s_teardown, k8s_registry_cleanup.

CLUSTERS. Never switch the active cluster yourself. If the person names another cluster, ask them to switch in Settings, Clusters. Every deploy names the cluster in its approval text. A production-looking cluster is read-only for you: stop, say so, offer a development cluster.

GPU: request nvidia.com/gpu only when k8s_get nodes shows it in allocatable; else run on CPU and say so.

PUBLISH only when asked: store_publish (private store, approval always asked).

FAILURES (one change, new tag, retry; after 3 tries park the row with the reason):
ImagePullBackOff: use the exact name deploy_load returned
CrashLoopBackOff: bug_report, fix CMD, env or port
Pending: lower requests, check the node
CreateContainerConfigError: missing Secret or ConfigMap, or a non-numeric USER
Readiness failing: containerPort, targetPort and $PORT equal
OOMKilled: raise the memory limit one step
Quota exceeded: lower replicas or requests

DONE when: rollout complete, Ready, restarts 0, HTTP 200 on /healthz, address given, rows done or parked, cluster named. Report: cluster, tag, address, rows, open B- bugs.

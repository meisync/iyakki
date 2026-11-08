# Our store on k3s (`dist/store-k3s/`)

The existing `iyakki-store-server` (signed catalog, immutable packages, publishing with a token) as a Deployment in its own namespace `iyakki-store` on the k3s of the PC that builds and boots IYAKKI. Plan: `docs/development/COMPONENT-UPDATES-PLAN.md`. Details of the server: `docs/STORE.md` section 13.

Files: `00-namespace.yaml` (namespace, ResourceQuota 1 CPU / 1 GiB requests and 2 CPU / 2 GiB limits, LimitRange), `10-pvc.yaml` (50 GiB, `local-path`), `20-secret.example.yaml` (shape only), `30-deployment.yaml`, `40-service.yaml` (ClusterIP `iyakki-store` and NodePort 30580). Everything carries `app.kubernetes.io/part-of: iyakki-store`. This repository holds no secret.

Use `KUBECONFIG=/etc/rancher/k3s/k3s.yaml` and check `kubectl get nodes` shows this PC before every apply (another PC's cluster may be your default).

## Status (2026-11-07)

Manifests were applied, but the pod could not start: this PC's k3s was started with node address 192.168.50.69 and the PC now has 192.168.50.103, so pods cannot reach the API server (`dial tcp 10.43.0.1:443: no route to host`) and the `local-path` provisioner cannot create the volume (PVC stays Pending). It affects every workload on this cluster, not only this one. Fix on the host (owner): give the PC back .69 (DHCP reservation) or restart k3s with `--node-ip 192.168.50.103` (a k3s config change). After that the steps below work unchanged.

## 1. Image (once per server version)

No tool is installed. Docker is present and builds a `FROM scratch` image around the static `target/x86_64-unknown-linux-musl/release/iyakki-store-server`:

```sh
B=$HOME/iyakki-work/store-k3s/build; mkdir -p $B/data && chmod 777 $B/data
cp target/x86_64-unknown-linux-musl/release/iyakki-store-server $B/iyakki
cat > $B/Dockerfile <<'EOF'
FROM scratch
COPY iyakki /iyakki
COPY --chown=10001:10001 data /data
USER 10001:10001
EXPOSE 8080
ENTRYPOINT ["/iyakki"]
CMD ["store-server", "--data", "/data", "--listen", "0.0.0.0:8080"]
EOF
cd $B && DOCKER_BUILDKIT=0 docker build -t localhost:5000/iyakki-store-server:0.1.0-alpha.1 .
docker push localhost:5000/iyakki-store-server:0.1.0-alpha.1     # the local Docker registry; this cluster's containerd mirrors localhost:5000 to it
# if the pod cannot pull (no registry route), import into the node's containerd instead (needs root):
docker save localhost:5000/iyakki-store-server:0.1.0-alpha.1 | sudo k3s ctr -n k8s.io images import -
```

## 2. Secret (host files, outside the repository)

```sh
S=$HOME/iyakki-work/store-k3s/secrets; mkdir -p $S; chmod 700 $S; umask 077
openssl rand -hex 32 > $S/catalog-seed      # the catalog key is derived from it inside the pod
openssl rand -hex 32 > $S/admin-token       # the publish / administrator token
export KUBECONFIG=/etc/rancher/k3s/k3s.yaml
kubectl apply -f dist/store-k3s/00-namespace.yaml
kubectl -n iyakki-store create secret generic store-secrets --from-file=catalog-seed=$S/catalog-seed --from-file=admin-token=$S/admin-token
kubectl -n iyakki-store label secret store-secrets app.kubernetes.io/part-of=iyakki-store
```

Keep a second copy of both files (losing the seed means a new key and every device must add the store again).

## 3. Deploy

```sh
kubectl apply -f dist/store-k3s/10-pvc.yaml -f dist/store-k3s/30-deployment.yaml -f dist/store-k3s/40-service.yaml
kubectl -n iyakki-store rollout status deploy/iyakki-store
```

## 4. URL, key and checks

URL on the LAN: `http://<this PC's LAN address>:30580` (find it with `ip route get 1.1.1.1`; at the time of writing 192.168.50.103). Inside the cluster: `http://iyakki-store.iyakki-store.svc:8080`. No TLS; integrity does not depend on it (everything is signed).

Key fingerprint to pin: `88c5 a12c cb93 fc03 f7d1 9476 3c50 7ca4` (catalog key `736370198ebf3a09ff315a66a4857cf22cc21ec89f3f085079a7a97fd9329180`, derived from the seed in the host file). Compare it with `curl -s http://<address>:30580/v1/info` before pinning.

```sh
curl -s http://<address>:30580/healthz ; curl -s http://<address>:30580/v1/info ; curl -s http://<address>:30580/catalog.json
```

## 5. Publish

The administrator token publishes as `admin`; give people their own with `kubectl -n iyakki-store exec deploy/iyakki-store -- /iyakki publisher-add --data /data --catalog-seed-file /secrets/catalog-seed --pub-id alice`. Limit today: packages up to 10 MiB (see `docs/development/COMPONENT-STORE-FINDINGS.md`).

```sh
iyakki pkg keygen mykey                                   # once: your signing key (keep it)
iyakki pkg pack my-project -o app.iyp --key mykey.key     # or: pkg build ... then pkg sign
export STORE_TOKEN=$(cat $HOME/iyakki-work/store-k3s/secrets/admin-token)
iyakki pkg publish --server http://<address>:30580 --token-env STORE_TOKEN app.iyp
```

Withdraw: `curl -X DELETE -H "Authorization: Bearer $STORE_TOKEN" "http://<address>:30580/v1/packages/<name>/<version>?reason=..."`.

## 6. Add it to a dev PC's STORE (documentation only; nothing has been done on a device)

STORE, Settings, Private store, Add a private store: type `http://<address>:30580`, check that the shown fingerprint is `88c5 a12c cb93 fc03 f7d1 9476 3c50 7ca4`, accept. The key is then pinned: a catalog signed by any other key is refused.

## 7. Backup and restore

The PV is a folder of the node (`/var/lib/rancher/k3s/storage/pvc-...`). With `reclaimPolicy: Delete`, **`kubectl delete ns iyakki-store` deletes the PV and all its data.** Back up first:

```sh
mkdir -p $HOME/iyakki-work/store-k3s/backups
kubectl -n iyakki-store exec deploy/iyakki-store -- /iyakki --help >/dev/null   # pod is up
# the image has no tar, so copy the folder out with kubectl cp from a helper pod that mounts the same claim:
kubectl -n iyakki-store run backup --restart=Never --image=busybox:1.36 --overrides='{"spec":{"securityContext":{"runAsUser":10001},"containers":[{"name":"backup","image":"busybox:1.36","command":["sleep","600"],"volumeMounts":[{"name":"d","mountPath":"/data"}]}],"volumes":[{"name":"d","persistentVolumeClaim":{"claimName":"store-data"}}]}}'
kubectl -n iyakki-store wait --for=condition=Ready pod/backup
kubectl -n iyakki-store exec backup -- tar czf - -C /data . > $HOME/iyakki-work/store-k3s/backups/store-$(date +%F).tar.gz
kubectl -n iyakki-store delete pod backup
```

(The claim is ReadWriteOnce on a single node, so the helper pod can share it. busybox must be pullable; otherwise use `tar` from any image you have.) Restore, run by the owner into an empty claim: start the same helper pod, then `kubectl -n iyakki-store exec -i backup -- tar xzf - -C /data < store-DATE.tar.gz`, delete the helper and restart the Deployment (`kubectl -n iyakki-store rollout restart deploy/iyakki-store`). The Secret (seed, token) is not in the archive; it is the host files in step 2.

## 8. Removal

`kubectl delete ns iyakki-store` removes the Deployment, Services, Secret, quota, claim and (local-path, reclaim Delete) the data. Optional: `docker rmi localhost:5000/iyakki-store-server:0.1.0-alpha.1`, `sudo k3s ctr -n k8s.io images rm localhost:5000/iyakki-store-server:0.1.0-alpha.1`, and the host folder `$HOME/iyakki-work/store-k3s`.

## Notes from the first deployment (2026-11-07)

* **The k3s node IP must match the host.** If the PC's LAN address changes (DHCP), pods lose the API server (`no route to host`) and volumes stay Pending. Fix: `node-ip` and `advertise-address` in `/etc/rancher/k3s/config.yaml` (then `systemctl restart k3s`), or a DHCP reservation on the router.
* **The server takes `--flag VALUE` as two words**, not `--flag=VALUE`; the Deployment args are written that way.
* **Verified on the cluster:** the pod runs non-root with a read-only root file system, `/healthz` and `/v1/info` answer on the NodePort from the LAN, and the data (`catalog.json`, `state.json`, `yanked.json`, `packages/`) is on the PVC's directory under the k3s storage path and survives a pod restart. Publishing from another machine and the signature check against the pinned key were run against a host-local server, not against this pod.

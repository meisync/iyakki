---
name: k3s-node-webapp
description: Write a Node.js web app or API that runs well on k3s: layout, a small node:http server with healthz and graceful shutdown, node:test tests, config, state, and the usual Node failures
triggers: node web app, node api, node server, express, fastify, nodejs k3s, npm start, node service, node container
---
Use for a Node.js web app or API on k3s. Build, ship, verify: k3s-build-and-deploy; Dockerfile: Node block of k3s-dockerfile-patterns; rules for any language: k3s-web-service-patterns. ACT yourself, never hand the person commands.

1. Layout (ONE folder passed as dir): server.js, package.json, package-lock.json (commit it; `npm install --package-lock-only` makes one), tests/*.test.js, public/ for static files, Dockerfile, .dockerignore, deploy/<name>.yaml. package.json: "type": "module", "engines": {"node": ">=22"}, scripts start = `node server.js`, test = `node --test`.
2. Start with node:http, no framework; add express or fastify only when really needed, pinned.
```
import http from "node:http";
const port = Number(process.env.PORT || 8080);
export const server = http.createServer((req, res) => {
  const { pathname } = new URL(req.url, "http://x");
  if (pathname === "/healthz") { res.writeHead(200, { "content-type": "text/plain" }); return res.end("ok"); }
  if (pathname === "/") { res.writeHead(200, { "content-type": "text/html; charset=utf-8" }); return res.end("<h1>Hello</h1>"); }
  res.writeHead(404); res.end("not found");
});
if (process.argv[1] === new URL(import.meta.url).pathname) {
  server.listen(port, "0.0.0.0", () => console.log(JSON.stringify({ msg: "listening", port })));
  process.on("SIGTERM", () => server.close(() => process.exit(0)));
}
```
3. Test first (the Dockerfile `test` stage runs `npm test`). Listen on port 0 so tests never clash:
```
import test from "node:test"; import assert from "node:assert/strict"; import { server } from "../server.js";
test("healthz answers ok", async () => {
  await new Promise((r) => server.listen(0, r));
  const res = await fetch(`http://127.0.0.1:${server.address().port}/healthz`);
  assert.equal(res.status, 200); server.close();
});
```
4. Config from process.env with a default; secrets only through vault_request, then a Secret and env valueFrom, never in files, images or logs. Log one JSON line per event to stdout.
5. State: the image is read-only in spirit; write only to an emptyDir or a volume at /data. Memory: limit 256Mi, NODE_OPTIONS=--max-old-space-size=192. One process per pod (no pm2, no cluster module); scale with replicas.
6. Static files: resolve the path, refuse anything outside public/, set the content type, never build HTML from user text, cap request bodies (1 MB).
7. Verify after the rollout: deploy_status, then a real request to /healthz and one page at the "reachable at" address. Browser features (camera, microphone, clipboard, service workers) need https or an address on 127.0.0.1: a NodePort over plain http cannot use them, so say that and ask the person to open the app through a forward on this computer instead of claiming it works.
8. No podman or docker (deploy_build says so): do not build or apply :latest. Use rule 15 of k3s-web-service-patterns: image node:22-alpine, server.js and public/ in a ConfigMap at /app, command `node /app/server.js`.
9. Failures: `Cannot find module` (a runtime dependency sits in devDependencies, move it to dependencies); ERR_REQUIRE_ESM (mixed import styles, keep "type": "module"); EADDRINUSE (use $PORT); CrashLoopBackOff at start (`node --check server.js`, then bug_report); readiness failing (listen on 0.0.0.0, containerPort = Service targetPort = $PORT); OOMKilled (raise the limit one step); slow start (readiness initialDelaySeconds 3).

Board: track steps with board_add (done only after a passed check). Bugs and failures: own board row with a B- id (never F-); bug_report captures logs; fix, verify, attach the passing run.

Example: "a small node page that shows the time, on k3s" -> server.js, tests, Dockerfile, deploy/clock.yaml; deploy_build target=test, deploy_build, deploy_load, k8s_apply, deploy_status; say the address.

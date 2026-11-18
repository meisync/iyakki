---
name: lang-javascript
description: Write, run and explain modern JavaScript (ES2021+, Node.js): syntax, functions, control flow, arrays/objects/Map/Set, strings, JSON, async errors, and best practices.
triggers: javascript, js, node, nodejs, npm, mjs, eslint, prettier, jest, async, promise
---
JavaScript (ES2021+) reference for writing and explaining code.

1. Run: `node app.js`, REPL `node`, syntax check `node --check app.js`; browser via `<script src="app.js">`. Layout: app.js, package.json (`npm init -y`), `npm install lodash`, tests/. ES modules: `"type": "module"` in package.json, then `import fs from "node:fs"; export function f() {}`.
2. Comments: `// line`, `/* block */`, doc `/** @param {number} x */`.
3. Variables: `const PI = 3.14;` (cannot rebind, default choice), `let n = 0;` (reassignable); never `var`. Types: number, string, boolean, null, undefined, bigint, symbol, object. Names camelCase, classes PascalCase, constants UPPER_CASE. Use `===` not `==`.
4. Functions:
```
function add(a, b = 0, ...rest) { return a + b; }
const sq = (x) => x * x;
```
Destructuring: `const {a, b} = obj; const [x, y] = arr;`.
5. Branching: `if (x > 0) {...} else if (x === 0) {...} else {...}`; ternary `c ? a : b`; `switch (k) { case "a": ...; break; default: ... }`. Defaults: `x ?? 5`, `obj?.deep?.val`. Falsy: 0, "", null, undefined, NaN.
6. Loops: `for (let i = 0; i < n; i++)`, `for (const x of arr)`, `for (const k in obj)` (keys; avoid for arrays), `while (c)`, `break`, `continue`. Functional: `arr.map(x => x*2)`, `.filter(...)`, `.reduce((s, x) => s + x, 0)`, `.forEach(...)`.
7. Data:
 - array: `a = [1, 2]`, `a.push(3)`, `a.pop()`, `a.shift()`, `a.splice(i, 1)`, `a.length`, `a.slice(1, 3)`, `a.includes(2)`, `a.indexOf(2)`, sort numbers `a.sort((x, y) => x - y)` (default sort is text).
 - object: `o = {k: 1}`, `o.j = 2`, `delete o.k`, `"k" in o`, `Object.keys(o)`, `Object.entries(o)`.
 - Map: `m = new Map(); m.set("k", 1); m.get("k"); m.has("k"); m.delete("k");`.
 - Set: `s = new Set([1]); s.add(2); s.has(1);`.
 - class: `class P { constructor(x) { this.x = x; } get2() { return this.x * 2; } }`.
8. Strings: `s.length`, `s.slice(1, 4)`, `s.substring(1, 4)`, `s.split(",")`, `parts.join(",")`, `s.trim()`, `s.replaceAll("a", "b")`, `s.indexOf("x")` (-1), `s.includes("x")`, `s.toUpperCase()`, template `${name}: ${n.toFixed(2)}`. Numbers: `parseInt("42", 10)`, `Number("1.5")`, `Number.isNaN(v)`. Regex: `/\d+/g`, `s.match(re)`, `re.test(s)`. JSON: `JSON.parse(t)`, `JSON.stringify(o, null, 2)`. Files (Node): `import {readFile} from "node:fs/promises"; await readFile("f.txt", "utf8")`. Dates: `new Date().toISOString()`.
9. Errors:
```
try { JSON.parse(t); } catch (e) { console.error(e.message); }
const r = await fetch(url); if (!r.ok) throw new Error("HTTP " + r.status);
```
Async: `async function f() { await g(); }`, `Promise.all([a(), b()])`, `.catch(...)`. Always handle rejected promises.
10. Best practice: use const/let, `===`; format with `prettier --write .`, lint `npx eslint .`, test `npm test` (jest or `node --test`); never `eval` or build HTML from user text (use `textContent`, not `innerHTML`, to avoid XSS); parameterize SQL; keep secrets in `process.env`; do not block the event loop; avoid mutating shared arrays; commit the lockfile and audit with `npm audit`.
11. When asked to write JavaScript, return one complete runnable Node script, run it if a tool is available (sandbox_run language javascript when the container cluster is available), and explain any error in one or two sentences.
12. Track multi-step work on the feature board (board_list/board_add/board_update, tests=, files=; done only after tests pass). ACT: write files and run builds and tests yourself; never ask the person to. Bugs/failures: own board row with a B- id (never F-); bug_report (failing pods) captures logs; fix, verify, attach the passing run.

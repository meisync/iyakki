---
name: lang-typescript
description: Write, compile and explain TypeScript: tsc and tsconfig, types, interfaces, generics, functions, control flow, collections, strings, errors, and best practices.
triggers: typescript, ts, tsc, tsx, tsconfig, interface, generics, npx, ts-node, tsx
---
TypeScript reference (JavaScript plus static types) for writing and explaining code.

1. Run: `npm install -D typescript`, `npx tsc --init`, type-check `npx tsc --noEmit`, compile `npx tsc` then `node dist/app.js`; run directly with `npx tsx app.ts`. Layout: src/app.ts, tsconfig.json (`"strict": true`, `"outDir": "dist"`), package.json. Types are erased at runtime.
2. Comments: `// line`, `/* block */`, `/** doc (TSDoc) @param x the value */`.
3. Variables: `const PI = 3.14;`, `let n: number = 0;`, `let s: string;`. Types: number, string, boolean, null, undefined, unknown, never, `any` (avoid). Arrays `number[]`, tuples `[string, number]`, unions `string | null`, literals `"a" | "b"`. Alias `type Id = string;`. `as const`, `readonly`. camelCase values, PascalCase types.
4. Functions:
```
function add(a: number, b = 0, ...rest: number[]): number { return a + b; }
const sq = (x: number): number => x * x;
```
Optional param `c?: string`. Generic: `function first<T>(a: T[]): T | undefined { return a[0]; }`.
5. Branching: `if/else`, ternary `c ? a : b`, `switch (k) { case "a": ...; break; default: ... }`. Narrowing: `if (typeof v === "string")`, `if (x !== null)`, `x ?? 5`, `obj?.val`. Exhaustive check: assign to `never` in default.
6. Loops: `for (let i = 0; i < n; i++)`, `for (const x of arr)`, `for (const [k, v] of Object.entries(o))`, `while`, `break`, `continue`; `arr.map(x => x * 2)`, `.filter(...)`, `.reduce(...)`.
7. Data:
```
interface User { id: number; name: string; email?: string }
const u: User = { id: 1, name: "a" };
```
 - array `const a: number[] = [1]`, `a.push(2)`, `a.splice(i, 1)`, `a.includes(2)`, `[...a].sort((x, y) => x - y)`.
 - `Record<string, number>` or `const m = new Map<string, number>()` with set/get/has/delete; `new Set<number>()` with add/has.
 - class: `class P { constructor(public x: number) {} }`; enum alternative: union literal types.
8. Strings: same as JavaScript: `s.length`, `s.slice(1, 4)`, `s.split(",")`, `a.join(",")`, `s.trim()`, `s.replaceAll("a", "b")` (tsconfig target/lib ES2021+), `s.includes("x")`, `s.toUpperCase()`, `${name}: ${n.toFixed(2)}`, `Number("1.5")`, `parseInt(s, 10)`, `re.test(s)`, `JSON.parse(t) as User` (unchecked cast, validate untrusted data), `JSON.stringify(o)`, `import {readFileSync} from "node:fs"`, `new Date().toISOString()`.
9. Errors:
```
try { run(); } catch (e) { if (e instanceof Error) console.error(e.message); }
```
Caught `e` is `unknown`; narrow before use. Async: `async function f(): Promise<number> {...}`, `await`, `Promise.all`.
10. Best practice: keep `strict` on; avoid `any` (use `unknown` plus narrowing); prefer `interface`/`type` over loose objects; `const` by default; format `npx prettier --write .`, lint `npx eslint .` (typescript-eslint), test `npx vitest` or `npx jest`; `@types/node` for Node; validate external JSON at runtime (the compiler cannot); avoid `!` non-null assertions and `as` casts; never put user text into `innerHTML` or SQL strings.
11. When asked to write TypeScript, return one complete file that type-checks with strict on, run it (`npx tsx`) if a tool is available, and explain compiler errors in one or two sentences.
12. Track multi-step work on the feature board (board_list/board_add/board_update, tests=, files=; done only after tests pass). ACT: write files and run builds and tests yourself; never ask the person to. Bugs/failures: own board row with a B- id (never F-); bug_report (failing pods) captures logs; fix, verify, attach the passing run.

---
name: lang-python
description: Write, run and explain Python 3 code: syntax, functions, control flow, lists/dicts/sets, strings, files, JSON, errors, and best practices (PEP 8, venv, pytest).
triggers: python, py, pip, venv, pytest, pep8, ruff, black, dict, list comprehension, python3
---
Python 3 reference for writing and explaining code.

1. Run: `python3 app.py`, REPL `python3`, check syntax `python3 -m py_compile app.py`. Layout: app.py, tests/test_app.py, requirements.txt; `python3 -m venv .venv` then `pip install requests`. Entry guard: `if __name__ == "__main__": main()`.
2. Comments: `# line`. Docstring first in a function: `"""Return x doubled."""` (also for modules, classes).
3. Variables: no declaration, dynamic types: `n = 3`, `name: str = "a"` (hints optional). Constants by convention UPPER_CASE. Names snake_case, classes CamelCase. Immutable: int, str, tuple; mutable: list, dict, set.
4. Functions:
```
def area(w, h=1, *rest, **opts) -> int:
    return w * h
sq = lambda x: x * x
```
Never use a mutable default (`def f(a=[])`); use None.
5. Branching: `if x > 0: ... elif x == 0: ... else: ...`; ternary `a if cond else b`; `match cmd: case "go": ... case _: ...` (3.10+). Truthiness: empty list, "", 0, None are false. Compare None with `is None`.
6. Loops: `for i in range(5):`, `for x in items:`, `for i, x in enumerate(items):`, `for k, v in d.items():`, `while cond:`, `break`, `continue`. Comprehension: `[x*x for x in xs if x > 0]`, `{k: v for k, v in pairs}`, generator `sum(x for x in xs)`.
7. Data:
 - list: `a = [1, 2]`, `a.append(3)`, `a.remove(1)`, `a.pop()`, `a[0]`, `a[1:3]`, `len(a)`, `sorted(a, key=len, reverse=True)`, `a.sort()`, `2 in a`.
 - dict: `d = {"k": 1}`, `d["j"] = 2`, `d.get("z", 0)`, `del d["k"]`, `"k" in d`.
 - set: `s = {1, 2}`, `s.add(3)`, `s.discard(1)`, `a | b`, `a & b`.
 - tuple: `t = (1, "a")`, `x, y = t`.
 - class: `@dataclass class P: x: int; y: int = 0` (from dataclasses import dataclass); plain `class A: def __init__(self, v): self.v = v`.
8. Strings: `len(s)`, `s[1:4]`, `s.split(",")`, `",".join(parts)`, `s.strip()`, `s.replace("a", "b")`, `s.find("x")` (-1 if none), `s.upper()`, `s.lower()`, `s.startswith("a")`, f-string `f"{name}: {n:.2f}"`. Numbers: `int("42")`, `float("1.5")` raise ValueError. Modules: `import re; re.findall(r"\d+", s)`, `re.search`, `re.sub`; `import json; json.loads(t); json.dumps(o)`; files `with open("f.txt", encoding="utf-8") as f: text = f.read()`; `from pathlib import Path; Path("f").read_text()`; `from datetime import datetime; datetime.now().isoformat()`; `import sys; sys.argv`.
9. Errors:
```
try:
    n = int(s)
except ValueError as e:
    print("bad", e)
finally:
    cleanup()
```
`raise ValueError("msg")`. Catch specific exceptions, never bare `except:`.
10. Best practice: follow PEP 8; format with `black .` or `ruff format`, lint with `ruff check`; test with `pytest`; use type hints; use `with` for files, locks, connections; never `eval`/`exec` untrusted input; use `subprocess.run([...], check=True)` with a list, not shell=True with user text; parameterized SQL (`cur.execute("... ?", (v,))`); keep secrets in env vars (`os.environ["KEY"]`); avoid mutating a list while iterating.
11. When asked to write Python, return one complete runnable script with a main guard, run it if a tool is available (sandbox_run language python when the container cluster is available), and explain any error in one or two sentences.
12. Track multi-step work on the feature board (board_list/board_add/board_update, tests=, files=; done only after tests pass). ACT: write files and run builds and tests yourself; never ask the person to. Bugs/failures: own board row with a B- id (never F-); bug_report (failing pods) captures logs; fix, verify, attach the passing run.

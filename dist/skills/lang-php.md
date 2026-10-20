---
name: lang-php
description: Write, run and explain PHP 8 code: php CLI, composer, types, functions, arrays, strings, exceptions, PDO and security best practices
triggers: php, composer, phpunit, artisan, laravel, phpcs, pdo, php.ini
---
Use this when writing or explaining PHP 8.

1. Run: `php main.php`; syntax check `php -l main.php`; dev server `php -S localhost:8000`; packages `composer require <vendor/pkg>`; tests `vendor/bin/phpunit`. Files start with `<?php` (omit the closing `?>`). Layout: public/index.php, src/, composer.json, vendor/. Use `declare(strict_types=1);` first.
2. Comments: `// line`, `# line`, `/* block */`, doc `/** @param int $a */`.
3. Variables: `$n = 5; $s = "hi"; const MAX = 10;` Types: int, float, string, bool, array, null, object; declare `int|string`, `?int`. Names: camelCase vars/functions, PascalCase classes, UPPER_CASE consts.
4. Functions: `function add(int $a, int $b = 0): int { return $a + $b; }` named args `add(b: 2, a: 1)`; variadic `function sum(int ...$xs)`; arrow fn `$sq = fn($x) => $x * $x;` closure `function($x) use ($y) {...}`.
5. Branch: `if ($a) {...} elseif ($b) {...} else {...}`; ternary `$a ? $x : $y`, null coalesce `$v ?? 'default'`; `switch ($c) { case 1: ...; break; default: ...; }`; `match` (strict, returns): `$r = match($n) { 1 => 'one', 2, 3 => 'few', default => 'many' };`
6. Loops: `for ($i = 0; $i < $n; $i++) {}`, `foreach ($xs as $v) {}`, `foreach ($m as $k => $v) {}`, `while ($c) {}`, `do {} while ($c);`, break/continue. Functional: `array_map(fn($x) => $x * 2, $xs)`, `array_filter($xs, fn($x) => $x > 1)`, `array_sum($xs)`.
7. Data:
- List: `$xs = [3, 1, 2]; $xs[] = 4; array_pop($xs); unset($xs[0]);` (keeps keys; `array_splice($xs, 0, 1)` reindexes) `count($xs); sort($xs); in_array(2, $xs, true);`
- Map: `$m = ['a' => 1]; $m['b'] = 2; isset($m['a']); array_key_exists('a', $m); unset($m['a']);` `usort($xs, fn($a, $b) => $a <=> $b);`
- Set: keys `$set[$x] = true; isset($set[$x]);`.
- Class: `class P { public function __construct(public string $name, public int $age = 0) {} }` `$p = new P('Al');` enum (8.1+) `enum Suit { case Hearts; }`
8. Strings: `strlen($s)`, `substr($s, 1, 3)`, `explode(',', $s)`, `implode(',', $xs)`, `trim($s)`, `str_replace('a', 'b', $s)`, `strpos($s, 'x')` (false if missing, compare with `!== false`), `str_contains($s, 'x')`, `strtoupper($s)`, `strtolower($s)`, interpolation `"Hi {$p->name} $n"`, `sprintf('%s %.2f', $a, $b)`. UTF-8: `mb_strlen`, `mb_substr`. Regex: `preg_match('/\d+/', $s, $m)`. Numbers: `(int)$s`, `is_numeric($s)`. Output: `echo $x, "\n";` `var_dump($x);` Files: `file_get_contents($p)`, `file_put_contents($p, $t)`. JSON: `json_encode($v)`, `json_decode($s, true)` (throwing: `json_decode($s, true, 512, JSON_THROW_ON_ERROR)`). Dates: `date('Y-m-d')`.
9. Errors: `try { ... } catch (InvalidArgumentException $e) { echo $e->getMessage(); } finally { }`; `throw new RuntimeException('msg');` `catch (Throwable $e)` as last resort.
10. Best practices:
- Style: PSR-12; check `vendor/bin/phpcs --standard=PSR12 src`, static analysis `phpstan`, tests PHPUnit.
- Use strict_types, types, `===` not `==`.
- Security: PDO prepared statements `$st = $pdo->prepare('SELECT * FROM t WHERE id = ?'); $st->execute([$id]);` never concatenate input into SQL; escape HTML output with `htmlspecialchars($s, ENT_QUOTES)`; no `eval`; avoid `exec` with input (`escapeshellarg`).
- Passwords: `password_hash()` / `password_verify()`; secrets from env (`getenv`), never in code.
- Autoload with Composer (`require 'vendor/autoload.php';`), display_errors off in production.
11. Assistant: when asked to write PHP, return a complete runnable script starting with `<?php`, run it if a tool is available (sandbox_run language php when the container cluster supports it), and explain any error in one or two sentences.
12. Track multi-step work on the feature board (board_list/board_add/board_update, tests=, files=; done only after tests pass). ACT: write files and run builds and tests yourself; never ask the person to. Bugs/failures: own board row with a B- id (never F-); bug_report (failing pods) captures logs; fix, verify, attach the passing run.

---
name: lang-java
description: Write, compile and explain Java: javac/java, classes, methods, control flow, collections, streams, strings, exceptions, and best practices (try-with-resources, JUnit).
triggers: java, javac, jar, jdk, maven, mvn, gradle, junit, arraylist, hashmap, jvm
---
Java reference (JDK 17+ style) for writing and explaining code.

1. Run: `javac Main.java` then `java Main`; single file `java Main.java`; jar `jar cfe app.jar Main *.class`, `java -jar app.jar`. One public class per file, file name equals class name. Entry: `public static void main(String[] args)`. Build tools: `mvn test`, `gradle test`.
2. Comments: `// line`, `/* block */`, `/** Javadoc. @param x the value @return the result */`.
3. Variables: `int n = 3; double d = 1.5; boolean ok = true; char c = 'a'; long big = 1L; String s = "hi";`. `final int MAX = 10;` constants `static final`. `var list = new ArrayList<String>();` (local inference). Names camelCase, classes PascalCase, constants UPPER_CASE. Compare strings with `.equals`, never `==`.
4. Methods:
```
static int add(int a, int b) { return a + b; }
static int sum(int... xs) { int t = 0; for (int x : xs) t += x; return t; }
Function<Integer, Integer> sq = x -> x * x;
```
Overload by parameter types; no default arguments. `void` returns nothing.
5. Branching: `if (x > 0) {...} else if (x == 0) {...} else {...}`; ternary `c ? a : b`; `switch (k) { case 1: ...; break; default: ... }`; modern `String r = switch (k) { case 1 -> "one"; default -> "many"; };`.
6. Loops: `for (int i = 0; i < n; i++)`, `for (String x : list)`, `while (c)`, `do {...} while (c);`, `break`, `continue`. Streams: `list.stream().filter(x -> x > 0).map(x -> x * 2).collect(Collectors.toList())`, `IntStream.range(0, 5)`.
7. Data (import java.util.*):
 - array: `int[] a = new int[5]; int[] b = {1, 2}; a.length; Arrays.sort(b); Arrays.toString(b);`.
 - List: `List<String> l = new ArrayList<>(); l.add("a"); l.get(0); l.remove(0); l.size(); l.contains("a"); Collections.sort(l); l.sort(Comparator.comparing(String::length));`.
 - Map: `Map<String, Integer> m = new HashMap<>(); m.put("k", 1); m.get("k"); m.getOrDefault("z", 0); m.containsKey("k"); m.remove("k"); for (var e : m.entrySet()) e.getKey();`.
 - Set: `Set<Integer> s = new HashSet<>(); s.add(1); s.contains(1);`.
 - record: `record Point(int x, int y) {}` then `new Point(1, 2).x()`. class: fields, constructor, getters.
8. Strings: `s.length()`, `s.substring(1, 4)`, `s.split(",")`, `String.join(",", list)`, `s.trim()` or `s.strip()`, `s.replace("a", "b")`, `s.indexOf("x")` (-1), `s.toUpperCase()`, `s.charAt(0)`, `s.equals(t)`, `String.format("%s: %.2f", name, n)`, `StringBuilder sb = new StringBuilder(); sb.append("a");`. Numbers: `Integer.parseInt("42")`, `Double.parseDouble("1.5")` throw NumberFormatException. Regex: `java.util.regex.Pattern.compile("\\d+").matcher(s)`. Files: `Files.readString(Path.of("f.txt"))` (java.nio.file). Dates: `java.time.LocalDate.now()`. JSON needs a library (Jackson, Gson).
9. Errors:
```
try { int n = Integer.parseInt(s); }
catch (NumberFormatException e) { System.out.println(e.getMessage()); }
finally { cleanup(); }
```
`throw new IllegalArgumentException("msg");`. Checked exceptions (IOException) must be caught or declared with `throws`.
10. Best practice: follow Google Java Style; test with JUnit 5 (`mvn test`); use try-with-resources `try (var r = Files.newBufferedReader(p)) {...}`; compare with `.equals`; prefer interfaces (`List`) as types; avoid null (`Optional`); PreparedStatement for SQL, never string concatenation; do not deserialize untrusted data; catch specific exceptions, never swallow them.
11. When asked to write Java, return one complete file with a `Main` class and `main` that compiles with `javac`, run it if a tool is available, and explain compiler errors in one or two sentences.
12. Track multi-step work on the feature board (board_list/board_add/board_update, tests=, files=; done only after tests pass). ACT: write files and run builds and tests yourself; never ask the person to. Bugs/failures: own board row with a B- id (never F-); bug_report (failing pods) captures logs; fix, verify, attach the passing run.

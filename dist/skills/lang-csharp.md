---
name: lang-csharp
description: Write, run and explain C# (.NET) code: dotnet CLI, types, methods, LINQ, collections, strings, exceptions, async and best practices
triggers: c#, csharp, dotnet, cs, csproj, nuget, linq, aspnet, .net
---
Use this when writing or explaining C# on modern .NET (6+).

1. Run: `dotnet new console -o app && cd app && dotnet run`; build `dotnet build`; test `dotnet test`; packages `dotnet add package <Name>`. Layout: App.csproj, Program.cs, one class per file. Top-level statements allow a Program.cs with no Main.
2. Comments: `// line`, `/* block */`, XML doc `/// <summary>text</summary>`.
3. Variables: `int n = 5; var s = "hi"; const int Max = 10; readonly` fields for run-once constants. Types: int, long, double, decimal, bool, char, string, object; nullable `int? x = null;`. Names: PascalCase types/methods/properties, camelCase locals, _camelCase private fields.
4. Methods: `static int Add(int a, int b = 0) => a + b;` named args `Add(b: 2, a: 1)`; varargs `params int[] xs`; generics `T Max<T>(T a, T b) where T : IComparable<T>`; lambda `Func<int,int> sq = x => x * x;`
5. Branch: `if (a) {...} else if (b) {...} else {...}`; ternary `a ? x : y`; `switch (c) { case 1: ...; break; default: ...; break; }`; expression `var r = n switch { 1 => "one", > 1 => "many", _ => "none" };`
6. Loops: `for (int i=0;i<n;i++)`, `foreach (var v in list)`, `while (c) {}`, `do {} while (c);`, break/continue. LINQ (using System.Linq): `list.Where(x => x > 1).Select(x => x * 2).ToList()`, `.Sum()`, `.Any()`, `.OrderBy(x => x)`.
7. Data:
- `var l = new List<int>{3,1,2}; l.Add(4); l.Remove(3); l.RemoveAt(0); l.Count; l.Sort(); l.Contains(2);` array `int[] a = {1,2};`
- `var d = new Dictionary<string,int>{["a"]=1}; d["b"]=2; d.TryGetValue("a", out var v); d.ContainsKey("a"); d.Remove("a"); foreach (var (k, val) in d) {}`
- `var h = new HashSet<int>{1}; h.Add(2); h.Contains(2);`
- Tuple `(int, string) t = (1, "x");` record `record Person(string Name, int Age);` class `class P { public int X { get; set; } }`
8. Strings: `s.Length`, `s.Substring(i, n)`, `s.Split(',')`, `string.Join(",", xs)`, `s.Trim()`, `s.Replace("a","b")`, `s.IndexOf("x")` (-1 if missing), `s.Contains("x")`, `s.ToUpper()`, `s.ToLower()`, interpolation `$"{a} and {b:F2}"`. Parse: `int.Parse(s)`, `int.TryParse(s, out var n)`. Regex: `System.Text.RegularExpressions.Regex.IsMatch(s, @"\d+")`. Files: `File.ReadAllText(p)`, `File.WriteAllText(p, t)`, `File.ReadAllLines(p)`. JSON: `JsonSerializer.Serialize(o)` / `Deserialize<T>(s)` (System.Text.Json). Dates: `DateTime.Now`, `DateTime.UtcNow`, `d.ToString("yyyy-MM-dd")`. Console: `Console.WriteLine(x); Console.ReadLine();`
9. Errors: `try { ... } catch (FormatException e) { Console.WriteLine(e.Message); } finally { }`, `throw new ArgumentException("msg");`, `using var f = File.OpenRead(p);` disposes automatically.
10. Best practices:
- Style: Microsoft C# conventions; format `dotnet format`; analyzers on build; tests xUnit or NUnit via `dotnet test`.
- Enable `<Nullable>enable</Nullable>`; handle null with `?.`, `??`.
- Prefer `TryParse`, `List<T>`/LINQ over manual arrays; `string.Join` or StringBuilder in loops.
- Use `using` for IDisposable; `async`/`await` with `Task`, never `.Result` or `.Wait()`.
- Security: parameterised SQL (`@p` parameters), never concatenate input into SQL or shell; no BinaryFormatter.
- Prefer immutable records and `readonly`; catch specific exceptions, not bare `Exception`.
11. Assistant: when asked to write C#, return a complete runnable example (Program.cs), run it if a tool is available (sandbox_run language csharp when the container cluster supports it), and explain any error in one or two sentences.
12. Track multi-step work on the feature board (board_list/board_add/board_update, tests=, files=; done only after tests pass). ACT: write files and run builds and tests yourself; never ask the person to. Bugs/failures: own board row with a B- id (never F-); bug_report (failing pods) captures logs; fix, verify, attach the passing run.

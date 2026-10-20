---
name: lang-cpp
description: Write, compile and explain C++ (C++17/20) code: g++/cmake, types, functions, STL containers, strings, errors, RAII and best practices
triggers: c++, cpp, g++, clang++, cmake, makefile, stl, vector, header, hpp, cc
---
Use this when writing or explaining C++. Snippets need C++17 unless marked C++20.

1. Run: `g++ -std=c++17 -Wall -Wextra -O2 main.cpp -o app && ./app`. Projects: CMakeLists.txt, `cmake -B build && cmake --build build`. Layout: src/*.cpp, include/*.h(pp), tests/.
2. Comments: `// line`, `/* block */`, doc `/** @brief ... */`.
3. Variables: `int n = 5; auto x = 2.5; const int MAX = 10; constexpr int K = 3;` Types: int, long long, double, bool, char, std::string, size_t. Names: snake_case vars/functions, PascalCase types.
4. Functions: `int add(int a, int b = 0) { return a + b; }` Pass big objects as `const std::string& s`. Lambda: `auto sq = [](int v){ return v*v; };` Templates: `template <typename T> T mx(T a, T b){ return a>b?a:b; }`
5. Branch: `if (a) {...} else if (b) {...} else {...}`; ternary `a ? x : y`; `switch (c) { case 1: ...; break; default: ...; }`
6. Loops: `for (int i=0;i<n;++i)`, `for (const auto& v : vec)`, `while (c) {}`, `do {} while (c);`, break/continue. Structured: `for (auto& [k,v] : m)`.
7. Data (include <vector> <map> <unordered_map> <set> <tuple> <algorithm>):
- `std::vector<int> v{3,1,2}; v.push_back(4); v.pop_back(); v.erase(v.begin()+1); v.size();`
- `std::sort(v.begin(), v.end());` membership: `std::find(v.begin(),v.end(),3)!=v.end()`
- `std::unordered_map<std::string,int> m; m["a"]=1; m.count("a"); m.erase("a");` ordered: std::map
- `std::set<int> s; s.insert(1); s.contains(1)` (C++20; else `s.count(1)`)
- `std::array<int,3> a{}; auto t = std::make_tuple(1,"x"); struct P{int x; int y;};`
8. Strings (<string>): `s.size()`, `s.substr(pos,len)`, `s.find("x")` (== std::string::npos if missing), `s.replace(pos,len,"y")`, `s += "z"`, `std::to_string(5)`, `std::stoi(s)`, `std::stod(s)` (throw on bad input). No built-in split/trim/join: loop with `std::istringstream` + `std::getline(in, part, ',')`. Upper: `std::transform(s.begin(),s.end(),s.begin(),::toupper)`. Format: `std::format("{} {}", a, b)` (C++20, <format>, needs GCC 13+) or `std::ostringstream`. Regex: `std::regex_search(s, std::regex("\\d+"))`. Output: `std::cout << x << '\n';` Files: `std::ifstream f("a.txt"); std::getline(f, line);` `std::ofstream`. Time: <chrono>. No JSON in std: use a library (nlohmann/json).
9. Errors: `try { ... } catch (const std::exception& e) { std::cerr << e.what(); }`, `throw std::runtime_error("msg");` Check `if (!f)` for streams. `std::optional<T>` for maybe-values (C++17).
10. Best practices:
- Style: C++ Core Guidelines; format `clang-format`, lint `clang-tidy`, compile with -Wall -Wextra.
- RAII: no raw new/delete; use `std::unique_ptr` (`std::make_unique<T>(...)`), containers, `std::string`.
- Prefer `const` and references; `std::string_view` for read-only params.
- Never use `using namespace std;` in headers; guard headers with `#pragma once`.
- Avoid buffer overflows: use `.at(i)`, no `gets`/`strcpy`/`sprintf`; validate all external input; never build shell commands from input.
- Do not use dangling references or iterators after `push_back`/`erase`; initialise all variables.
- Test with sanitizers: `-fsanitize=address,undefined -g`; unit tests with GoogleTest or Catch2.
11. Assistant: when asked to write C++, return a complete runnable program with main(), run it if a tool is available (sandbox_run language cpp when the container cluster supports it), and explain any error in one or two sentences.
12. Track multi-step work on the feature board (board_list/board_add/board_update, tests=, files=; done only after tests pass). ACT: write files and run builds and tests yourself; never ask the person to. Bugs/failures: own board row with a B- id (never F-); bug_report (failing pods) captures logs; fix, verify, attach the passing run.

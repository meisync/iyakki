//! Examples to try the assistant with, put in place once when an administrator first signs in: a sample MCP server, three
//! sample documents for RAG and three sample skills. Each can be deleted where it lives (MCP tile, RAG's Documents list,
//! SKILLS tile); once deleted it does not come back, because this runs only once.

use iyakki_agent::mcp::StoredServer;
use iyakki_agent::skills::Scope;
use iyakki_sec::policy::Policy;

use crate::shell::ShellEnv;

const MARKER: &str = "samples.done";

pub const DOCS: [(&str, &str); 3] = [
    (
        "sample-iyakki-quick-start.md",
        "# IYAKKI quick start (sample)\n\nIYAKKI is an operating system for agents. Its tiles:\n\n- CHAT talks to a model. With the agent switched on it can read files, call web APIs, run ssh, aws or gcloud, play music, plan your calendar and run code in a container.\n- RAG searches the documents you keep (this is one of them).\n- MCP connects to Model Context Protocol servers; the `iyakki-demo` server is a sample with echo, add, now and dice tools.\n- SKILLS holds short instructions the assistant follows when a request matches.\n- CLI has a terminal and a list of command-line tools you can install (kubectl, helm, terraform, aws, gcloud, az, arduino-cli, esptool).\n- CALENDAR keeps events and alarms; the chat can read and plan in it.\n- RADIO plays internet radio in many languages.\n\nDelete this document from RAG, Documents when you do not need it.\n",
    ),
    (
        "sample-home-lab-cheatsheet.md",
        "# Home lab cheat sheet (sample)\n\n## Kubernetes with k3s\n- Install k3s in CLI, CLIs, then start the cluster there.\n- `kubectl get pods -A` lists everything running.\n- `kubectl logs <pod>` shows what a pod printed.\n- `kubectl delete pod <pod>` removes a pod; its deployment starts a new one.\n\n## ssh\n- `ssh user@host` opens a shell; keys live in ~/.ssh.\n- `ssh -L 8080:localhost:80 user@host` forwards a port.\n\n## Ollama\n- `ollama pull <model>` downloads a model; `ollama list` shows the ones you have.\n- NET SCAN finds Ollama servers on the local network and can add them to Models.\n",
    ),
    (
        "sample-python-practice.md",
        "# Python practice problems (sample)\n\n1. FizzBuzz: print 1 to 30; multiples of 3 print Fizz, of 5 print Buzz, of both FizzBuzz.\n2. Palindromes: write is_palindrome(text) that ignores case, spaces and punctuation.\n3. Word count: count how often each word appears in a sentence and print the top three.\n4. Fibonacci: print the first 15 numbers of the sequence with a loop, then with recursion.\n5. Primes: list the primes below 100 with the sieve of Eratosthenes.\n\nAsk the chat to run your solution in a Python container and show the output.\n",
    ),
];

pub const SKILLS: [(&str, &str); 3] = [
    (
        "sample-explain-simply",
        "---\nname: sample-explain-simply\ndescription: Sample: explain a topic step by step in plain language with an example\ntriggers: explain, simply, eli5, what is\n---\nWhen asked to explain something, answer in short numbered steps, avoid jargon, define any term you must use, and finish with one small concrete example. Ask whether to go deeper.\n",
    ),
    (
        "sample-python-practice",
        "---\nname: sample-python-practice\ndescription: Sample: help someone practise Python by running their code in a container\ntriggers: python, practice, practise, run code, exercise\n---\nWhen someone wants to practise or test Python, write or take their program and run it with sandbox_run (language python), starting the container cluster first if it is not running. Show the output, explain any error in one or two sentences, fix it and run it again. Suggest the next small exercise.\n",
    ),
    (
        "sample-meeting-notes",
        "---\nname: sample-meeting-notes\ndescription: Sample: turn meeting notes into decisions and action items, and offer to schedule them\ntriggers: meeting, notes, minutes, action items\n---\nSummarise the notes as: Decisions, Action items (who and by when) and Open questions. Then offer to put each dated action item in the calendar with calendar_add.\n",
    ),
];

/// Puts the examples in place once. Only for accounts that manage the system (they own the shared things).
pub fn seed(env: &ShellEnv, user: &str, policy: &Policy) {
    if !policy.manage_settings {
        return;
    }
    let svc = &env.services;
    let marker = svc.dir().join(MARKER);
    if marker.exists() {
        return;
    }
    // a sample MCP server (this program again, with --mcp-demo)
    let d = iyakki_agent::mcp::demo_spec();
    let _ = svc.mcp.add(StoredServer { name: d.name, command: d.command, args: d.args, sample: true });
    // sample documents in the shared folder; RAG indexes them when it opens
    let shared = std::path::Path::new(&env.cfg.rag_docs).join("shared");
    // (the documents folder is only created under a data folder that exists, so nothing is made in odd places)
    let base_exists = std::path::Path::new(&env.cfg.rag_docs).parent().is_some_and(|p| p.is_dir());
    if base_exists && std::fs::create_dir_all(&shared).is_ok() {
        for (file, text) in DOCS {
            let p = shared.join(file);
            if !p.exists() {
                let _ = std::fs::write(p, text);
            }
        }
    }
    // sample skills, shared with everyone
    for (_, md) in SKILLS {
        let _ = svc.skills.add(Scope::Shared, md, true, true);
    }
    let _ = std::fs::write(&marker, "samples were added once; delete them where they live\n");
    env.audit.record(user, "samples.seed", "mcp server, 3 documents, 3 skills");
}

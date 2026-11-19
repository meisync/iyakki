#!/usr/bin/env python3
"""F-SPLIT-03 census of the UI/agent contract (docs/development/UI-RPC-CENSUS.md).

Parses every `pub trait` of crates/iyakki-agent-api/src, classifies each method by the shape of its signature, and counts the call sites of
each method name in the non-test code of iyakki-apps and iyakki-shell. Usage:
    dist/split/rpc_census.py --json OUT.json     the parsed table (also read by gen_rpc.py)
    dist/split/rpc_census.py --md docs/development/UI-RPC-CENSUS.md
Method names that several traits share (`get`, `list`) are counted once per name, so a count is an upper bound for each of them.
"""
import re, sys, glob, os, json, collections

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))

def strip(s):
    s = re.sub(r'//[^\n]*', '', s)
    return re.sub(r'/\*.*?\*/', '', s, flags=re.S)

def parse():
    res = []
    for f in sorted(glob.glob(ROOT + '/crates/iyakki-agent-api/src/*.rs')):
        t = strip(open(f).read())
        for m in re.finditer(r'^pub trait (\w+)[^{;]*\{', t, flags=re.M):
            name = m.group(1); i = m.end(); d = 1; j = i
            while d and j < len(t):
                d += (t[j] == '{') - (t[j] == '}'); j += 1
            body = t[i:j - 1]
            d = 0; cur = ''; items = []; br = 0
            for c in body:
                if d == 0 and c in '[(': br += 1
                if d == 0 and c in '])': br -= 1
                if c == '{':
                    d += 1
                    if d == 1: items.append(cur + ' @BODY'); cur = ''; continue
                if c == '}':
                    d -= 1
                    if d == 0: continue
                if d >= 1: continue
                if c == ';' and br == 0: items.append(cur); cur = ''; continue
                cur += c
            for it in items:
                body_ = it.rstrip().endswith('@BODY'); it = it.replace('@BODY', '')
                mm = re.search(r'\bfn\s+(\w+)\s*(<[^>]*>)?\s*\((.*)\)\s*(->\s*(.*))?$', it.strip(), flags=re.S)
                if not mm: continue
                fn, gen, args, _, ret = mm.groups()
                res.append(dict(file=os.path.basename(f), trait=name, fn=fn, gen=gen or '', args=' '.join(args.split()), ret=' '.join((ret or '').split()), body=body_))
    return res

# classes, in the order the design document explains them
CLASSES = [
    ('S',  'serializable: owned or borrowed data both ways (strings, numbers, bytes, serde types)'),
    ('IO', 'in/out: a `&mut T` argument the callee changes (sent, changed, sent back)'),
    ('H',  'returns a handle kept by the caller (`Arc<dyn Trait>`, `Box<dyn Trait>`): a remote object id with release on drop'),
    ('E',  'takes a closure or a callback (`Fn`, `FnMut`, audit lines, progress): an event subscription or a core-side default'),
    ('R',  'borrowed accessor (`&T`, `&Arc<..>`, `Option<&..>`) of a member the UI then calls: a root object id, or a cached snapshot'),
    ('X',  'takes an I/O object of the UI (`&dyn Fetch`, `&dyn Runner`, `&dyn Streamer`, `Arc<dyn ChatBackend>`, `Arc<dyn Tools>`): the core uses its own; the argument leaves the signature'),
    ('A',  'downcast or composition-root only (`dyn Any`, `as_any`): never on the wire'),
]

def classify(r):
    a, ret, gen = r['args'], r['ret'], r['gen']
    a_nofn = re.sub(r'dyn Fn(Mut|Once)?\([^)]*\)(\s*->\s*[\w<>&, ]+)?(\s*\+\s*(Send|Sync))*', 'CLOSURE', a)
    if re.search(r'dyn (std::)?any::Any|dyn Any|\bas_any', a + ret + r['fn']): return 'A'
    if re.search(r'dyn (Fn|FnMut|FnOnce)\b|impl Fn|AuditFn|AuditLine|SftpLogin|SftpLookup|SharedOpen|TaskList|\bCLOSURE\b', a) or gen: return 'E'
    if re.search(r'dyn\s+(\w+::)*(Fetch|Runner|Streamer|ChatBackend|Tools|HostApi|Os|Fleet|Router|Kube|ExternalAgent|WizardBackend|Reg\w*|CueOut|Upstream|Synth|Mic|Speaker|TurnLog|LessonHost|DashHost|HeaderValues)\b', a): return 'X'
    if re.search(r'&mut\s', a_nofn): return 'IO'
    if re.search(r'^&|Option<&', ret): return 'R'
    if re.search(r'(Arc|Box)<(Mutex<)?dyn', ret) or re.search(r'Option<(Arc|Box)<dyn', ret): return 'H'
    return 'S'

def uicalls():
    cnt = collections.Counter()
    for d in ('crates/iyakki-apps/src', 'crates/iyakki-shell/src'):
        for r, _, fs in os.walk(os.path.join(ROOT, d)):
            if '/tests' in r + '/': continue
            for f in fs:
                if not f.endswith('.rs') or f.endswith('_tests.rs') or f == 'tests.rs': continue
                t = open(os.path.join(r, f)).read()
                k = t.find('\n#[cfg(test)]')
                if k >= 0: t = t[:k]
                for m in re.finditer(r'\.(\w+)\s*(?:::<[^>]*>)?\(', t): cnt[m.group(1)] += 1
    return cnt

STD = set('get set push take remove insert len iter next extend clone new default list add delete start stop send is_empty contains find map open close read write run name status update load save lock refresh check info state fmt eq cmp'.split())

def build():
    res = parse(); calls = uicalls()
    owners = collections.defaultdict(set)
    for r in res:
        if not re.match(r'Services(B\d|FacetB\d|Handle)$', r['trait']): owners[r['fn']].add(r['trait'])
    for r in res:
        r['cls'] = classify(r)
        # a name that several traits (or std types: get, push, take) share cannot be attributed: 0 and flagged
        own = owners[r['fn']] or {r['trait']}
        r['amb'] = len(own) > 1 or r['fn'] in STD
        r['calls'] = 0 if r['amb'] else calls.get(r['fn'], 0)
    return res

def md(res):
    tot = len(res); by = collections.Counter(r['cls'] for r in res)
    o = ['# UI RPC census (F-SPLIT-03, alpha.7)', '',
         'Generated by `dist/split/rpc_census.py --md docs/development/UI-RPC-CENSUS.md`; do not edit by hand. Every `pub trait` of `crates/iyakki-agent-api/src` is parsed;',
         'each method is put in the first class of the list below that fits its signature. Call sites are the occurrences of `.name(` in the non-test code of',
         '`iyakki-apps` and `iyakki-shell` (a name that several traits or std types share, such as `get` or `push`, cannot be attributed and counts 0; so a count is a lower bound).', '',
         '## Totals', '', '| class | meaning | methods | share |', '|---|---|---:|---:|']
    for k, why in CLASSES:
        o.append(f'| {k} | {why} | {by[k]} | {100*by[k]/tot:.1f}% |')
    o.append(f'| | all | {tot} | 100% |'); o.append('')
    sv = by['S'] + by['IO'] + by['H'] + by['R']
    o += [f'Patterns that carry a class: S and IO as plain calls, H as remote objects, R as root objects or snapshots, E as event subscriptions or core-side defaults, X by leaving the argument out (the core owns that I/O). Class A is never remote. "Expressible by the planned patterns without moving a service": S + IO + H + R = {sv} of {tot} ({100*sv/tot:.1f}%); E + X + A = {by["E"]+by["X"]+by["A"]} ({100*(by["E"]+by["X"]+by["A"])/tot:.1f}%) need a decision per method (see UI-RPC-DESIGN.md section 5).', '']
    ts = len({r['trait'] for r in res})
    o += [f'{ts} traits. `ServicesHandle` alone has {sum(1 for r in res if r["trait"]=="ServicesHandle")} methods.', '', '## By trait', '', '| file | trait | S | IO | H | E | R | X | A | all | UI call sites |', '|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|']
    g = collections.defaultdict(list)
    for r in res: g[(r['file'], r['trait'])].append(r)
    for (f, t), ms in sorted(g.items()):
        c = collections.Counter(m['cls'] for m in ms)
        o.append(f'| {f} | {t} | ' + ' | '.join(str(c[k]) for k in ('S', 'IO', 'H', 'E', 'R', 'X', 'A')) + f' | {len(ms)} | {sum(m["calls"] for m in ms)} |')
    o += ['', '## The 20% of methods the UI calls most', '', 'Methods sorted by call sites in the UI (only names that belong to one trait and are not std-like names such as `get` or `push` are attributed; the rest count 0 and are marked in the first column of the by-trait table as shared names).', '', '| calls | trait::method | class |', '|---:|---|---|']
    top = sorted(res, key=lambda r: -r['calls'])
    n20 = max(1, tot // 5)
    cut = top[n20 - 1]['calls']
    for r in top[:n20]:
        pass
    shown = [r for r in top if r['calls'] >= max(cut, 1)]
    for r in shown[:60]:
        o.append(f"| {r['calls']} | {r['trait']}::{r['fn']} | {r['cls']} |")
    cs = collections.Counter(r['cls'] for r in top[:n20])
    o += ['', f'The top {n20} methods (20%) by call sites are in these classes: ' + ', '.join(f'{k} {cs[k]}' for k in ('S', 'IO', 'H', 'E', 'R', 'X', 'A')) + f' (the first {min(60,len(shown))} are listed). Threshold: {cut} call sites.', '',
          '## Non-serializable methods, listed', '']
    for k in ('E', 'X', 'A', 'R', 'IO'):
        ms = [r for r in res if r['cls'] == k]
        o.append(f'### Class {k} ({len(ms)})'); o.append('')
        o.append(', '.join(f"`{r['trait']}::{r['fn']}`" for r in ms)); o.append('')
    return '\n'.join(o) + '\n'

if __name__ == '__main__':
    res = build()
    if '--json' in sys.argv: json.dump(res, open(sys.argv[sys.argv.index('--json') + 1], 'w'), indent=0)
    if '--md' in sys.argv: open(sys.argv[sys.argv.index('--md') + 1], 'w').write(md(res))
    c = collections.Counter(r['cls'] for r in res); print(len(res), dict(c))

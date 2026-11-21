#!/usr/bin/env python3
"""serde_sweep.py LOG [--apply]: read the compiler output of `cargo check -p iyakki-ui-rpc` after gen_rpc.py and list (or derive) the types
the generated proxy needs: `serde::Serialize`/`Deserialize` for a type on the wire, `Default` for a type a lost call has to answer with
(`lost_default!`). Finds `pub struct|enum Name` under crates/*/src (a module hint from the path narrows an ambiguous name)."""
import re, sys, glob, os, collections
ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
log = open(sys.argv[1]).read()
apply = '--apply' in sys.argv
need = collections.OrderedDict()   # name -> set of wants
CTX = {}
for m in re.finditer(r"the trait bound `([^`]+?): (serde::Serialize|serde::Deserialize<'de>|serde::de::DeserializeOwned|Serialize|Deserialize<'de>|Lost)` is not satisfied\n\s*--> (\S+?):(\d+)", log):
    ty, what, spanf = m.group(1), m.group(2), m.group(3)
    what = what.replace('serde::', '')
    cm = re.match(r'crates/([^/]+)/', spanf)
    if cm and cm.group(1) != 'iyakki-ui-rpc': CTX[ty] = (cm.group(1), spanf)
    if ty.startswith('dyn ') or '<' in ty or '(' in ty or '&' in ty: continue
    w = {'Serialize': 'ser', 'Lost': 'lost'}.get(what, 'de')
    need.setdefault(ty, set()).add(w)
files = [f for f in glob.glob(ROOT + '/crates/*/src/**/*.rs', recursive=True) if '/iyakki-ui-rpc/' not in f]
cache = {}
def text(f):
    if f not in cache: cache[f] = open(f).read()
    return cache[f]
GEN = open(ROOT + '/crates/iyakki-ui-rpc/src/gen_remote.rs').read() + open(ROOT + '/crates/iyakki-ui-rpc/src/gen_serve.rs').read()
SHORT = collections.defaultdict(set)   # short name -> full paths imported by the generated code
def expand(u):
    u = ' '.join(u.split())
    i = u.find('{')
    if i < 0: return [u]
    pre = u[:i]; depth = 0; j = i; parts = []; cur = ''
    for k in range(i + 1, len(u)):
        c = u[k]
        if c == '{': depth += 1
        if c == '}':
            if depth == 0: parts.append(cur); break
            depth -= 1
        if c == ',' and depth == 0: parts.append(cur); cur = ''; continue
        cur += c
    out = []
    for p_ in parts:
        p_ = p_.strip()
        if p_: out += expand(pre + p_)
    return out
for m in re.finditer(r'^\s*(?:pub )?use ([^;]*);', GEN, flags=re.M):
    for full in expand(m.group(1)):
        it = full.split(' as ')[-1].split('::')[-1].strip()
        SHORT[it].add(full.split(' as ')[0].strip())
defs = collections.defaultdict(list)   # (crate dir, name) -> [(file, match)]
pat = re.compile(r'^pub (struct|enum) (\w+)', re.M)
for f in files:
    cr = re.search(r'/crates/([^/]+)/', f).group(1)
    s_ = text(f)
    for m in pat.finditer(s_): defs[(cr, m.group(2))].append((f, m))
def path_of(f, name):
    cr = re.search(r'/crates/([^/]+)/src/(.*)\.rs$', f)
    mod = cr.group(2).replace('/mod', '').split('/')
    mod = [x for x in mod if x not in ('lib', 'main')]
    return '::'.join([cr.group(1).replace('-', '_')] + mod + [name])
def attrs_of(f, m):
    s_ = text(f); start = m.start()
    while start > 0:
        prev = s_.rfind('\n', 0, start - 1) + 1
        if re.match(r'^[ \t]*(#\[|///)', s_[prev:start]): start = prev
        else: break
    return start
FULLPATH = {}
def find(ty):
    if ty in CTX:
        cr, spanf = CTX[ty]
        name = ty.split('::')[-1]; mods = ty.split('::')[:-1]
        cand = defs.get((cr, name), [])
        if len(cand) > 1 and mods:
            h = [c for c in cand if re.search(r'/' + re.escape(mods[-1]) + r'(\.rs|/)', c[0])]
            if h: cand = h
        if len(cand) > 1:
            h = [c for c in cand if c[0].endswith(spanf.split(':')[0].split('crates/')[1].split('/', 1)[1])]
            if h: cand = h
        if not cand:
            alls = [d for (c2, n2), d in defs.items() if n2 == name and c2 in ('iyakki-shared','iyakki-core-api','iyakki-agent-api','iyakki-memx','iyakki-tile-api','iyakki-sec','iyakki-accounts','iyakki-clock','iyakki-extagentx','iyakki-data')]
            cand = [x for d in alls for x in d]
            if len(cand) == 1: cr = re.search(r'/crates/([^/]+)/', cand[0][0]).group(1)
        if len(cand) == 1:
            FULLPATH[ty] = cr.replace('-', '_') + '::' + name
            return cand[0], None
        return None, 'ctx ' + str(len(cand))
    full = [ty] if '::' in ty else sorted(SHORT.get(ty, []))
    if not full:
        DC = ('iyakki-shared','iyakki-core-api','iyakki-agent-api','iyakki-memx','iyakki-tile-api','iyakki-sec','iyakki-accounts','iyakki-clock','iyakki-extagentx','iyakki-data')
        cand = [x for (c2, n2), d in defs.items() if n2 == ty.split('::')[-1] and c2 in DC for x in d]
        if len(cand) == 1:
            FULLPATH[ty] = re.search(r'/crates/([^/]+)/', cand[0][0]).group(1).replace('-', '_') + '::' + ty
            return cand[0], None
        return None, 'no import for short name; %d candidates %s' % (len(cand), [os.path.relpath(c[0], ROOT) for c in cand][:4])
    res = []
    for fp in full:
        parts = fp.split('::')
        if parts[0] in ('crate', 'self', 'super', 'std', 'core', 'alloc'): continue
        cr = parts[0].replace('_', '-')
        if not os.path.isdir(ROOT + '/crates/' + cr): continue
        cand = defs.get((cr, parts[-1]), [])
        mods = [x for x in parts[1:-1]]
        pick = cand
        if len(cand) > 1 and mods:
            h = [c for c in cand if re.search(r'/' + re.escape(mods[0]) + r'(\.rs|/)', c[0]) or any(re.search(r'/' + re.escape(mm) + r'(\.rs|/)', c[0]) for mm in mods)]
            if len(h) >= 1: pick = h
        if len(pick) > 1 and cr != 'iyakki-shared':   # prefer own file named like the module
            pass
        res.append((fp, pick))
    got = {(f, m.start()) for _, pk in res for f, m in pk}
    if not res: return None, 'not a workspace type'
    if len(got) == 1:
        fp, pk = next((r for r in res if r[1]))
        FULLPATH[ty] = fp
        return pk[0], None
    if not got: return None, 'no definition'
    return None, 'ambiguous ' + str(sorted(os.path.relpath(f, ROOT) for f, _ in got))
changes = collections.defaultdict(list)
LOSTS = []
OBJECTS = []
for ty, ws in need.items():
    hit, why = find(ty)
    if not hit: print('SKIP', ty, sorted(ws), why); continue
    f, m = hit
    st = attrs_of(f, m)
    attrs = text(f)[st:m.start()]
    add = []
    body = text(f)[m.start():m.start() + 4000]
    body = body[:body.find('\n}\n') + 3] if '\n}\n' in body else body
    if re.search(r'\bdyn |Sender<|Receiver<|JoinHandle|Mutex<|RwLock<|Atomic|\bInstant\b|Condvar|\bChild\b|\bFile\b|Arc<|Box<|\bfn\(', body) and 'ser' in ws:
        print('OBJECT', ty, os.path.relpath(f, ROOT)); OBJECTS.append(ty); continue
    if re.search(r'^\s*impl Default for ' + re.escape(ty.split('::')[-1]) + r'\b', text(f), flags=re.M): has_default = True
    else: has_default = False
    if ('ser' in ws or 'de' in ws) and not ('Serialize' in attrs and 'Deserialize' in attrs): add += ['serde::Serialize', 'serde::Deserialize']
    if 'lost' in ws and 'Default' not in attrs and not has_default:
        if m.group(1) == 'struct': add.append('Default')
        else: print('ENUM NEEDS DEFAULT', ty, os.path.relpath(f, ROOT))
    print(('derive ' if add else 'ok ') + ty, add, os.path.relpath(f, ROOT))
    if add: changes[f].append((st, m.start(), add))
    if 'lost' in ws and (m.group(1) == 'struct'): LOSTS.append(path_of(f, ty.split('::')[-1]))
if apply:
    for f, lst in changes.items():
        s_ = text(f)
        for st, en, add in sorted(lst, key=lambda x: -x[0]):
            attrs = s_[st:en]
            dm = re.search(r'#\[derive\(([^)]*)\)\]', attrs)
            if dm: new = attrs.replace(dm.group(0), '#[derive(' + dm.group(1).rstrip().rstrip(',') + ', ' + ', '.join(add) + ')]', 1)
            else: new = attrs + '#[derive(' + ', '.join(add) + ')]\n'
            s_ = s_[:st] + new + s_[en:]
        open(f, 'w').write(s_)

if apply and LOSTS:
    p_ = ROOT + '/crates/iyakki-ui-rpc/src/lost_default.rs'
    old = set(re.findall(r'^\s+([\w:]+),?$', open(p_).read(), flags=re.M)) if os.path.exists(p_) else set()
    allp = sorted(old | set(LOSTS))
    open(p_, 'w').write('// Types whose quiet value of a lost call is `Default` (written by dist/split/serde_sweep.py; add by hand below the marker).\nlost_default!(\n' + ',\n'.join('    ' + x for x in allp) + '\n);\n')

open('/tmp/x3/objects.txt', 'a').write('\n'.join(OBJECTS) + ('\n' if OBJECTS else ''))

# a derived `Default` whose field has none: take the derive back (the type needs a hand-written `Lost` in lost_manual.rs)
MANUAL = []
if apply:
    for m in re.finditer(r"the trait bound `([^`]+?): std::default::Default` is not satisfied\n\s*--> (\S+?):(\d+)", log):
        fp = ROOT + '/' + m.group(2)
        if not os.path.exists(fp) or '/iyakki-ui-rpc/' in fp: continue
        lines = open(fp).read().split('\n'); ln = int(m.group(3)) - 1
        i = ln
        while i >= 0 and not re.match(r'^pub (struct|enum) (\w+)', lines[i]): i -= 1
        if i < 0: continue
        name = re.match(r'^pub (?:struct|enum) (\w+)', lines[i]).group(1)
        j = i - 1
        while j >= 0 and lines[j].lstrip().startswith(('#[', '///')):
            if 'derive(' in lines[j] and 'Default' in lines[j]:
                lines[j] = re.sub(r',\s*Default\b|\bDefault,\s*', '', lines[j]); MANUAL.append((name, m.group(2)))
                open(fp, 'w').write('\n'.join(lines)); break
            j -= 1
    if MANUAL:
        print('MANUAL LOST NEEDED', MANUAL)
        p_ = ROOT + '/crates/iyakki-ui-rpc/src/lost_default.rs'
        t = open(p_).read()
        for name, f in MANUAL: t = re.sub(r'^\s+[\w:]+::' + name + r',?\n', '', t, flags=re.M)
        open(p_, 'w').write(t)

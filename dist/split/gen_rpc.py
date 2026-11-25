#!/usr/bin/env python3
"""F-SPLIT-03: generate the RPC proxy of the facet traits (docs/development/UI-RPC-DESIGN.md).

From each trait named in `dist/split/rpc-traits.txt` (or `--all-s`: every trait whose methods are all class S or H, see rpc_census.py) this writes
  crates/iyakki-ui-rpc/src/gen_remote.rs   `Remote<Trait>`: the trait implemented over the connection (client, inside the UI program)
  crates/iyakki-ui-rpc/src/gen_serve.rs    `Obj` (the kinds of object the core hands out) and `serve`: the dispatcher of the core
A trait is generated whole or not at all: a trait with one method of class E, X, A, R or IO is listed as skipped with the method that stops it.
Usage: dist/split/gen_rpc.py [--all-s] [--list] [--check]   (run from anywhere in the repository)
"""
import re, sys, os, collections, importlib.util

HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location('rpc_census', os.path.join(HERE, 'rpc_census.py'))
census = importlib.util.module_from_spec(spec); spec.loader.exec_module(census)
ROOT = census.ROOT
OUT = ROOT + '/crates/iyakki-ui-rpc/src'

def split_args(a):
    out, d, cur = [], 0, ''
    a = a.replace('->', '\x00')
    for c in a:
        if c in '<([': d += 1
        if c in '>)]': d -= 1
        if c == ',' and d == 0: out.append(cur.strip().replace('\x00', '->')); cur = ''; continue
        cur += c
    if cur.strip(): out.append(cur.strip().replace('\x00', '->'))
    return out

def owned(ty):
    """(owned type for the wire, expression turning the decoded value `x` into the borrowed argument)"""
    ty = ty.strip()
    m = re.match(r'^&(?:\'\w+\s+)?(.*)$', ty)
    if m:
        t = m.group(1).strip()
        if t.startswith('mut '): return t[4:].strip(), 'x'
        if t == 'str': return 'String', '&x'
        if t == '[&str]': return 'Vec<String>', '&x.iter().map(String::as_str).collect::<Vec<_>>()'
        if t.startswith('['): return 'Vec<%s>' % t[1:-1].strip(), '&x'
        if t.endswith('Path') and 'PathBuf' not in t: return 'std::path::PathBuf', '&x'
        return t, '&x'
    if re.match(r'^Vec<&str>$', ty): return 'Vec<String>', 'x.iter().map(String::as_str).collect::<Vec<_>>()'
    m = re.match(r'^Option<\(&str,\s*&str\)>$', ty)
    if m: return 'Option<(String, String)>', 'x.as_ref().map(|(a, b)| (a.as_str(), b.as_str()))'
    m = re.match(r'^Option<&(?:\'\w+\s+)?(.*)>$', ty)
    if m:
        t = m.group(1).strip()
        if t == 'str': return 'Option<String>', 'x.as_deref()'
        if t.startswith('['): return 'Option<Vec<%s>>' % t[1:-1].strip(), 'x.as_deref()'
        if t.endswith('Path'): return 'Option<std::path::PathBuf>', 'x.as_deref()'
        return 'Option<%s>' % t, 'x.as_ref()'
    return ty, 'x'

def last(name): return name.split('::')[-1].strip()

# `StoreApi: StorePublish`: the supertrait's methods are served through the subtrait's object
SUPERS = {'StoreApi': ['StorePublish']}
# borrowed accessors with a default body that are still read once from the core and kept
CACHED_OK = {'StoreApi::dir'}
STORE_TRAITS = {'StoreApi'}

def kind_of(arg, fn=''):
    """What the core supplies instead of an argument of the store traits (None: the argument is data): `fetch` and `audit` are the core's own, `actor`
    and `policy` are read again from the core's accounts by the person's name (the UI's claim is never used)."""
    ty = arg.split(':', 1)[1].strip() if ':' in arg else ''
    if re.match(r'^&dyn\s+(\w+::)*Fetch$', ty): return 'fetch'
    if re.match(r'^&dyn\s+Fn\(&str,\s*&str\)$', ty): return 'audit'
    if re.match(r'^&(\w+::)*Actor$', ty): return 'actor'
    if re.match(r'^&(\w+::)*Policy$', ty): return 'policy'
    if re.match(r'^&(std::path::)?Path$', ty) and arg.split(':', 1)[0].strip().lstrip('_') == 'dir': return 'dir_w' if re.search(r'bump|changelog', fn) else 'dir_r'
    return None

def handle_of(ret):
    m = re.match(r'^Result<\s*(?:std::sync::)?(Arc|Box)<dyn\s+([\w:]+)\s*>\s*,\s*(.+)>$', ret)
    if m: return ('result', m.group(1), last(m.group(2)), m.group(3).strip())
    m = re.match(r'^(Option<)?\s*(?:std::sync::)?(Arc|Box)<dyn\s+([\w:]+)\s*(?:\+[\w\s+]*)?>(>)?$', ret)
    if not m: return None
    return (bool(m.group(1)), m.group(2), last(m.group(3)))

def uses(path):
    t = census.strip(open(path).read())
    # `crate::` means the crate the trait file is in (iyakki-agent-api, or a leaf crate a trait moved to)
    me = re.search(r'/crates/([^/]+)/', path).group(1).replace('-', '_')
    out = []
    for m in re.finditer(r'^use\s+([^;]*?);', t, flags=re.M | re.S):
        u = ' '.join(m.group(1).split())
        u = re.sub(r'\bcrate::', me + '::', u)
        u = re.sub(r'^crate\b', me, u)
        if u.startswith('super') or u.startswith('self'): continue
        out.append('use ' + u + ';')
    return out

def main():
    res = census.build()
    by = collections.OrderedDict()
    for r in res: by.setdefault((r['file'], r['trait']), []).append(r)
    allow = None
    lst = os.path.join(HERE, 'rpc-traits.txt')
    if '--all-s' not in sys.argv and os.path.exists(lst):
        allow = {l.split('#')[0].strip() for l in open(lst) if l.split('#')[0].strip()}
    names = {t for (_, t) in by}
    # methods that carry a live object (an `Arc<HostVoice>`, a registry, a closure) are answered `not served` by the proxy (rpc-unserved.txt)
    unserved = set()
    up = os.path.join(HERE, 'rpc-unserved.txt')
    if os.path.exists(up): unserved = {l.split('#')[0].strip() for l in open(up) if l.split('#')[0].strip()}
    # methods written by hand in served_apps.rs (rpc-manual.txt): the client calls `served_apps::c_<Trait>_<fn>(self, args)`, the core's end is `served_apps::s_<Trait>_<fn>(t, reg, a)`
    manual = set()
    mp = os.path.join(HERE, 'rpc-manual.txt')
    if os.path.exists(mp): manual = {l.split('#')[0].strip() for l in open(mp) if l.split('#')[0].strip()}
    # a supertrait's methods are served and called through the subtrait's object (`StoreApi: StorePublish`): one object, one impl block per trait
    for (f, t) in list(by):
        for sup in SUPERS.get(t, []):
            k = [k for k in by if k[1] == sup][0]
            by[(f, t)] = by[(f, t)] + [dict(m, sup=sup) for m in by[k]]
    for (_, t), ms in by.items():
        for m in ms:
            if t + '::' + m['fn'] in unserved: m['cls'] = 'E'
            if t + '::' + m['fn'] in manual: m['manual'] = True
    # the store traits: the core's own fetch and audit stand in for the UI's, and the person is read again from the core's accounts (D-SPLIT-03-STORE)
    for (_, t), ms in by.items():
        if t not in STORE_TRAITS: continue
        for m in ms:
            m['sup'] = m.get('sup')
            ks = [kind_of(a, m['fn']) for a in split_args(m['args']) if ':' in a]
            m['kinds'] = ks
            if m.get('manual') or t + '::' + m['fn'] in unserved or not any(ks): continue
            rest = ', '.join(a for a in split_args(m['args']) if ':' not in a or kind_of(a, m['fn']) in (None, 'dir_r', 'dir_w'))
            if m['cls'] in ('X', 'E', 'S') and census.classify(dict(m, args=rest, gen='')) == 'S':
                m['cls'] = 'S'; m['wire'] = True
    # supertraits and `self:` receivers decide whether a trait can be an object at all
    src = {}
    for f in {f for (f, _) in by}: src[f] = census.strip(open(census.path_of(f)).read())
    ok, skipped = [], []
    for (f, t), ms in by.items():
        if allow is not None and t not in allow: continue
        head = re.search(r'pub trait %s\b([^{]*)\{' % t, src[f]).group(1)
        why = None
        if '<' in head or re.search(r':\s*[^+]*?(?<!Send)(?<!Sync)\s*(\+|$)', head.replace('Send', '').replace('Sync', '').replace('+', ' ').replace(':', ' ').strip() and head) and False: why = 'generic or supertrait'
        if 'Sync' not in head or 'Send' not in head: why = 'not `Send + Sync`'
        for m in ms:
            if not m['args'].lstrip().startswith('&self') and not re.match(r'^self:\s*Arc<Self>', m['args'].strip()): why = why or f"{m['fn']} has no `&self` receiver"
            if any(':' not in a and not re.match(r'^&?\s*(mut\s+)?self\b', a) for a in split_args(m['args'])): why = why or f"{m['fn']} has a signature the parser cannot read"
            if m['gen'] and t + '::' + m['fn'] not in unserved and not m.get('manual'): why = why or f"{m['fn']} is generic"
            if m['cls'] == 'H':
                h = handle_of(m['ret'])
                if h and h[2] not in names and (m['body'] or h[0]):
                    # a handle of a trait that is not served: the trait's own default answers, or the quiet value (None / Err) of the answer
                    m['mode'] = 'default' if m['body'] else 'unserved'; m['cls'] = 'E'
                elif not h or h[2] not in names: why = why or f"{m['fn']} returns a handle of a trait that is not served ({m['ret']})"
                elif h[2] == t and False: pass
            elif m['cls'] == 'IO' and t + '::' + m['fn'] in IO_OK:
                m['mode'] = 'io'
            elif m['cls'] != 'S':
                if m['cls'] == 'R' and (not m['body'] or t + '::' + m['fn'] in CACHED_OK) and m['args'].strip() == '&self' and re.match(r'^&(?!dyn\b)(?!\w*\s*dyn)[\w:]+$', m['ret']) and 'Arc<' not in m['ret'] and 'Engine' not in m['ret'] and 'PolicyBook' not in m['ret']: m['mode'] = 'cached'
                elif m['body']: m['mode'] = 'default'
                elif re.match(r'^(&|Option<&|impl\b)', m['ret']) or m['cls'] == 'A' and not m['ret']: why = why or f"{m['fn']} is class {m['cls']} with no default and a borrowed answer"
                else: m['mode'] = 'unserved'
        (skipped if why else ok).append((f, t, ms, why))
    # handles may only point at generated traits: drop traits whose handles point at a skipped trait, until stable
    changed = True
    while changed:
        changed = False
        okn = {t for (_, t, _, _) in ok}
        for item in list(ok):
            for m in item[2]:
                if m['cls'] == 'H' and handle_of(m['ret'])[2] not in okn:
                    if m['body'] or handle_of(m['ret'])[0]:
                        m['mode'] = 'default' if m['body'] else 'unserved'; m['cls'] = 'E'; changed = True; continue
                    ok.remove(item); skipped.append(item[:3] + (f"{m['fn']} returns a handle of {handle_of(m['ret'])[2]}, which is not generated",)); changed = True; break
    if '--list' in sys.argv:
        print('generated', len(ok), 'traits', sum(len(i[2]) for i in ok), 'methods')
        for f, t, ms, w in skipped: print('skipped', t, '-', w)
        return
    gen_remote(ok); gen_serve(ok)
    if '--check' in sys.argv:
        if STALE: print('stale (run dist/split/gen_rpc.py):', ', '.join(STALE)); sys.exit(1)
        print('fresh'); return
    print('generated', len(ok), 'traits,', sum(len(i[2]) for i in ok), 'methods; skipped', len(skipped), 'traits')

STALE = []

# methods with ONE `&mut T` argument that the callee changes and that are served by sending the value, running the call on the core's copy and sending the
# changed value back (class IO of the census): the type must derive serde
IO_OK = {'SessionApi::save'}

def emit(path, text):
    """Write a generated file; with `--check` write nothing and remember the ones that differ."""
    if '--check' in sys.argv:
        if open(path).read() != text: STALE.append(os.path.basename(path))
    else:
        open(path, 'w').write(text)

def prelude(files):
    o = ['// GENERATED by dist/split/gen_rpc.py (F-SPLIT-03). Do not edit: change the script or the trait and run it again.', '#![allow(unused_imports, unused_variables, clippy::all, non_snake_case, dead_code)]', '']
    return o

def modname(f): return 'g_' + f.replace('.rs', '')

def modpath(f):
    """The Rust path of the module that holds the trait file `f`: a file of iyakki-agent-api, or a leaf crate a trait moved to (rpc_census.MOVED_FILES)."""
    p = census.path_of(f)
    crate = re.search(r'/crates/([^/]+)/', p).group(1).replace('-', '_')
    return crate + '::' + os.path.basename(p)[:-3]

def gen_remote(ok):
    o = prelude(None)
    o += ['use std::sync::Arc;', 'use crate::{Lost, Rpc};', '']
    files = collections.OrderedDict()
    for f, t, ms, _ in ok: files.setdefault(f, []).append((t, ms))
    names = {t for (_, t, _, _) in ok}
    for f, items in files.items():
        o.append('mod %s {' % modname(f)); o.append('    use super::*;'); o.append('    use iyakki_agent_api::*;'); o.append('    use %s::*;' % modpath(f))
        for u in uses(census.path_of(f)): o.append('    ' + u)
        for t, ms in items:
            o.append(f'    /// `{t}` over the connection (object `obj` of the core).')
            cached = [m for m in ms if m.get('mode') == 'cached']
            cf = ''.join(f', c_{m["fn"]}: std::sync::OnceLock<{"String" if m["ret"] == "&str" else "std::path::PathBuf" if m["ret"] == "&Path" else m["ret"][1:]}>' for m in cached)
            o.append(f'    pub struct Remote{t} {{ pub(crate) rpc: Rpc, pub(crate) obj: u64{cf} }}')
            o.append(f'    impl Remote{t} {{ pub(crate) fn new(rpc: Rpc, obj: u64) -> Self {{ Remote{t} {{ rpc, obj{"".join(", c_" + m["fn"] + ": Default::default()" for m in cached)} }} }} }}')
            o.append(f'    impl Drop for Remote{t} {{ fn drop(&mut self) {{ self.rpc.release(self.obj); }} }}')
            blocks = [(t, [m for m in ms if not m.get('sup')])] + [(sp, [m for m in ms if m.get('sup') == sp]) for sp in SUPERS.get(t, [])]
            for bt, bms in blocks:
                o.append(f'    impl {bt} for Remote{t} {{')
                for m in bms:
                    if m.get('mode') == 'default' and not m.get('manual'): continue
                    args = split_args(m['args'])
                    params = [a for a in args if not re.match(r'^&?\s*(mut\s+)?self\b', a)]
                    decl = []; names_ = []
                    for p in params:
                        n, ty = p.split(':', 1); n = n.strip().lstrip('_') or 'a'; n = 'a_' + n if n in ('type', 'ref', 'match', 'fn', 'in') else n
                        decl.append(f'{n}: {ty.strip()}'); names_.append(n)
                    me = re.search(r'/crates/([^/]+)/', census.path_of(f)).group(1).replace('-', '_')  # `crate::` is the crate of the trait's file
                    m = dict(m, ret=re.sub(r'\bcrate::', me + '::', m['ret']))
                    decl = [re.sub(r'\bcrate::', me + '::', d) for d in decl]
                    ret = m['ret'] or '()'
                    recv = 'self: Arc<Self>' if re.match(r'^self:\s*Arc<Self>', m['args'].strip()) else '&self'
                    sig = f'        fn {m["fn"]}{m["gen"]}({recv}{"".join(", " + d for d in decl)})' + (f' -> {m["ret"]}' if m['ret'] else '') + ' {'
                    o.append(sig)
                    if m.get('wire'):
                        # the core supplies its own fetch and audit and re-reads the person and the policy; only the person's name crosses for an actor
                        kinds = [kind_of(p_, m['fn']) for p_ in params]
                        wire = [('&' + n_ + '.user') if k_ == 'actor' else n_ for n_, k_ in zip(names_, kinds) if k_ not in ('fetch', 'audit', 'policy')]
                        argv = 'serde_json::json!([' + ', '.join(wire) + '])'
                    else:
                        argv = 'serde_json::json!([' + ', '.join(names_) + '])'
                    if m.get('manual'):
                        o.append(f'            crate::served_apps::c_{t}_{m["fn"]}(self{"".join(", " + n for n in names_)})')
                    elif m.get('mode') == 'cached':
                        ty = 'String' if m['ret'] == '&str' else 'std::path::PathBuf' if m['ret'] == '&Path' else m['ret'][1:]
                        deref = '.as_str()' if m['ret'] == '&str' else '.as_path()' if m['ret'] == '&Path' else ''
                        o.append(f'            self.c_{m["fn"]}.get_or_init(|| self.rpc.call::<{ty}>("{t}", self.obj, "{m["fn"]}", serde_json::json!([])).unwrap_or_else(|e| <{ty} as Lost>::lost(&e))){deref}')
                    elif m.get('mode') == 'io':
                        mut_i = [i for i, p_ in enumerate(params) if re.match(r'^[\w ]+:\s*&mut\s', p_)][0]
                        mty = params[mut_i].split(':', 1)[1].strip()[len('&mut'):].strip()
                        sent = ', '.join(('&*' + n) if i == mut_i else n for i, n in enumerate(names_))
                        o.append(f'            match self.rpc.call::<({ret}, {mty})>("{t}", self.obj, "{m["fn"]}", serde_json::json!([{sent}])) {{ Ok((r, back)) => {{ *{names_[mut_i]} = back; r }} Err(e) => <{ret} as Lost>::lost(&e) }}')
                    elif m.get('mode') == 'unserved':
                        o.append(f'            let _ = ({"".join("&" + n + ", " for n in names_)});')
                        # a wrapper (`fn wrap(&self, base: Arc<dyn Tools>, ..) -> Arc<dyn Tools>`): not served means it wraps nothing
                        same = [n for n, p in zip(names_, params) if p.split(':', 1)[1].strip() == ret.strip()]
                        if same and not re.match(r'^(Result|Option)<', ret): o.append(f'            {same[0]}')
                        else: o.append(f'            <{ret} as Lost>::lost("this call is not served over the UI socket")')
                    elif m['cls'] == 'H':
                        hh = handle_of(m['ret'])
                        opt, kind, sub = hh[:3]
                        if opt == 'result':
                            err = hh[3]
                            sub_mod = [fx for fx, its in files.items() if any(tt == sub for tt, _ in its)][0]
                            mk = f'super::{modname(sub_mod)}::Remote{sub}::new(self.rpc.clone(), id)'
                            o.append(f'            match self.rpc.call::<Result<u64, {err}>>("{t}", self.obj, "{m["fn"]}", {argv}).unwrap_or_else(|e| <Result<u64, {err}> as Lost>::lost(&e)) {{ Ok(id) => Ok({kind}::new({mk})), Err(e) => Err(e) }}')
                            o.append('        }')
                            continue
                        call = f'self.rpc.call::<{"Option<u64>" if opt else "u64"}>("{t}", self.obj, "{m["fn"]}", {argv})'
                        mk = f'Remote{sub}::new(self.rpc.clone(), id)'
                        wrap = f'{kind}::new({mk})'
                        sub_mod = [fx for fx, its in files.items() if any(tt == sub for tt, _ in its)][0]
                        wrap = wrap.replace(f'Remote{sub}', f'super::{modname(sub_mod)}::Remote{sub}')
                        if opt:
                            o.append(f'            match {call} {{ Ok(Some(id)) => Some({wrap}), _ => None }}')
                        else:
                            # a lost core: an object that does not exist, so each call on it answers as a lost call does
                            lostwrap = wrap.replace(', id)', ', u64::MAX)')
                            o.append(f'            match {call} {{ Ok(id) => {wrap}, Err(_) => {lostwrap} }}')
                    else:
                        o.append(f'            self.rpc.call::<{ret}>("{t}", self.obj, "{m["fn"]}", {argv}).unwrap_or_else(|e| <{ret} as Lost>::lost(&e))')
                    o.append('        }')
                o.append('    }')
        o.append('}'); o.append('')
    o.append('pub use self::exports::*;'); o.append('mod exports {')
    for f, items in files.items():
        for t, _ in items: o.append(f'    pub use super::{modname(f)}::Remote{t};')
    o.append('}'); o.append('')
    # `Obj::remote`: a root object of a named kind, for the UI to start from
    o.append('/// A remote object of the named trait (the `iface` the core knows it by), or `None` when that trait is not generated.')
    o.append('pub fn remote_any(iface: &str, rpc: &Rpc, obj: u64) -> Option<RemoteAny> {')
    o.append('    Some(match iface {')
    for f, items in files.items():
        for t, _ in items: o.append(f'        "{t}" => RemoteAny::{t}(Arc::new(Remote{t}::new(rpc.clone(), obj))),')
    o.append('        _ => return None,'); o.append('    })'); o.append('}')
    o.append('/// One of the generated remote objects, by trait.'); o.append('pub enum RemoteAny {')
    for f, items in files.items():
        for t, _ in items: o.append(f'    {t}(Arc<dyn {modpath(f)}::{t}>),')
    o.append('}')
    emit(OUT + '/gen_remote.rs', '\n'.join(o) + '\n')

def gen_serve(ok):
    o = prelude(None)
    o += ['use std::sync::Arc;', 'use serde_json::Value;', 'use crate::{de, ser, Registry};', '']
    files = collections.OrderedDict()
    for f, t, ms, _ in ok: files.setdefault(f, []).append((t, ms))
    o.append('use iyakki_agent_api::*;')
    for f in files:
        o.append('mod %s { use super::*;' % modname(f)); o.append('    use %s::*;' % modpath(f))
        for u in uses(census.path_of(f)): o.append('    ' + u)
        o.append('    pub(crate) trait Dummy {}')
        for t, ms in files[f]:
            o.append(f"    pub(crate) fn serve_{t}(t: &Arc<dyn {t}>, reg: &crate::Registry<'_>, method: &str, a: Value) -> Result<Value, String> {{")
            o.append('        match method {')
            for m in ms:
                if m.get('manual'):
                    o.append(f'            "{m["fn"]}" => crate::served_apps::s_{t}_{m["fn"]}(t, reg, a),')
                    continue
                if m.get('mode') in ('default', 'unserved'): continue
                args = split_args(m['args'])
                params = [a for a in args if not re.match(r'^&?\s*(mut\s+)?self\b', a)]
                tys, exprs, names_ = [], [], []
                uservar, pend = None, []
                for i, p in enumerate(params):
                    n, ty = p.split(':', 1); k_ = kind_of(p, m['fn']) if m.get('wire') else None
                    if k_ == 'fetch': exprs.append('&*reg.fetch()?'); continue
                    if k_ == 'audit': exprs.append('&*reg.audit()'); continue
                    if k_ == 'policy': pend.append(len(exprs)); exprs.append(None); continue
                    if k_ == 'actor':
                        names_.append(f'x{i}'); tys.append('String'); exprs.append(f'&reg.actor(&x{i})?'); continue
                    ow, ex = owned(ty.strip())
                    me = re.search(r'/crates/([^/]+)/', census.path_of(f)).group(1).replace('-', '_')  # `crate::` is the crate of the trait's file
                    ow = re.sub(r'\bcrate::', me + '::', ow); ex = re.sub(r'\bcrate::', me + '::', ex)
                    names_.append(f'x{i}'); tys.append(ow); exprs.append(ex.replace('x', f'x{i}', 1) if ex.startswith('x') else ex.replace('x', f'x{i}'))
                    if k_ in ('dir_r', 'dir_w'): exprs[-1] = f'&reg.project_dir(&x{i}, {"true" if k_ == "dir_w" else "false"})?'
                    if n.strip().lstrip('_') == 'user': uservar = f'x{i}'
                for ix in pend:
                    assert uservar, m['fn']
                    exprs[ix] = f'&reg.policy_of(&{uservar})?'
                o.append(f'            "{m["fn"]}" => {{')
                if names_:
                    pat = '(' + ''.join(n + ', ' for n in names_) + ')'
                    ty = '(' + ''.join(t_ + ', ' for t_ in tys) + ')'
                    o.append(f'                let {pat}: {ty} = de(a)?;')
                if m.get('mode') == 'io':
                    mut_i = [i for i, p_ in enumerate(params) if re.match(r'^[\w ]+:\s*&mut\s', p_)][0]
                    o.pop()
                    # rebuild the decode line with the changed value bound mutably
                    pat = '(' + ''.join(('mut ' if i == mut_i else '') + n + ', ' for i, n in enumerate(names_)) + ')'
                    ty = '(' + ''.join(t_ + ', ' for t_ in tys) + ')'
                    o.append(f'                let {pat}: {ty} = de(a)?;')
                    ex2 = [('&mut ' + names_[i]) if i == mut_i else e for i, e in enumerate(exprs)]
                    o.append(f'                let r = t.{m["fn"]}(' + ', '.join(ex2) + ');')
                    o.append(f'                ser(&(r, {names_[mut_i]}))')
                    o.append('            }')
                    continue
                call = f't.{m["fn"]}(' + ', '.join(exprs) + ')'
                if re.match(r'^self:\s*Arc<Self>', m['args'].strip()): call = f't.clone().{m["fn"]}(' + ', '.join(exprs) + ')'
                if m['cls'] == 'H':
                    opt, kind, sub = handle_of(m['ret'])[:3]
                    conv = 'h' if kind == 'Arc' else 'Arc::from(h)'
                    if opt == 'result': o.append(f'                ser(&{call}.map(|h| reg.add(Obj::{sub}({conv}))))')
                    elif opt: o.append(f'                ser(&{call}.map(|h| reg.add(Obj::{sub}({conv}))))')
                    else: o.append(f'                {{ let h = {call}; ser(&reg.add(Obj::{sub}({conv}))) }}')
                else:
                    o.append(f'                ser(&{call})')
                o.append('            }')
            o.append(f'            other => Err(format!("{t} has no call {{other}}")),')
            o.append('        }'); o.append('    }')
        o.append('}')
    o.append('/// The kinds of object the core hands out: one per generated trait.')
    o.append('#[derive(Clone)]'); o.append('pub enum Obj {')
    for f, items in files.items():
        for t, _ in items: o.append(f'    {t}(Arc<dyn {modpath(f)}::{t}>),')
    o.append('    /// The whole `ServicesHandle` of the core (gen_services.py): its plain methods and the handles it hands out.')
    o.append('    ServicesHandle(Arc<crate::ServicesRoot>),')
    o.append('    /// A hand-written object (served.rs, D-SPLIT-03-SERVED): the traits that are not in iyakki-agent-api or cannot be generated.')
    o.append('    Served(Arc<dyn crate::served::Served>),')
    o.append('}')
    o.append('impl Obj {'); o.append('    pub fn iface(&self) -> &\'static str {'); o.append('        match self {')
    for f, items in files.items():
        for t, _ in items: o.append(f'            Obj::{t}(_) => "{t}",')
    o.append('            Obj::ServicesHandle(_) => "ServicesHandle",')
    o.append('            Obj::Served(s) => s.iface(),')
    o.append('        }'); o.append('    }')
    o.append("    pub(crate) fn serve(&self, reg: &crate::Registry<'_>, method: &str, a: Value) -> Result<Value, String> {"); o.append('        match self {')
    for f, items in files.items():
        for t, _ in items: o.append(f'            Obj::{t}(t) => {modname(f)}::serve_{t}(t, reg, method, a),')
    o.append('            Obj::ServicesHandle(t) => crate::gen_services::serve_services(t, reg, method, a),')
    o.append('            Obj::Served(s) => s.serve(reg, method, a),')
    o.append('        }'); o.append('    }'); o.append('}')
    emit(OUT + '/gen_serve.rs', '\n'.join(o) + '\n')

if __name__ == "__main__":
    main()

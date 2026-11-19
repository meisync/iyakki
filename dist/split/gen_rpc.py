#!/usr/bin/env python3
"""F-SPLIT-03: generate the RPC proxy of the facet traits (docs/development/UI-RPC-DESIGN.md).

From each trait named in `dist/split/rpc-traits.txt` (or `--all-s`: every trait whose methods are all class S or H, see rpc_census.py) this writes
  crates/iyakki-ui-rpc/src/gen_remote.rs   `Remote<Trait>`: the trait implemented over the connection (client, inside the UI program)
  crates/iyakki-ui-rpc/src/gen_serve.rs    `Obj` (the kinds of object the core hands out) and `serve`: the dispatcher of the core
A trait is generated whole or not at all: a trait with one method of class E, X, A, R or IO is listed as skipped with the method that stops it.
Usage: dist/split/gen_rpc.py [--all-s] [--list]   (run from anywhere in the repository)
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
        if t == 'str': return 'String', '&x'
        if t.startswith('['): return 'Vec<%s>' % t[1:-1].strip(), '&x'
        if t.endswith('Path') and 'PathBuf' not in t: return 'std::path::PathBuf', '&x'
        return t, '&x'
    m = re.match(r'^Option<&(?:\'\w+\s+)?(.*)>$', ty)
    if m:
        t = m.group(1).strip()
        if t == 'str': return 'Option<String>', 'x.as_deref()'
        if t.startswith('['): return 'Option<Vec<%s>>' % t[1:-1].strip(), 'x.as_deref()'
        if t.endswith('Path'): return 'Option<std::path::PathBuf>', 'x.as_deref()'
        return 'Option<%s>' % t, 'x.as_ref()'
    return ty, 'x'

def last(name): return name.split('::')[-1].strip()

def handle_of(ret):
    m = re.match(r'^(Option<)?\s*(Arc|Box)<dyn\s+([\w:]+)\s*>(>)?$', ret)
    if not m: return None
    return (bool(m.group(1)), m.group(2), last(m.group(3)))

def uses(path):
    t = census.strip(open(path).read())
    out = []
    for m in re.finditer(r'^use\s+([^;]*?);', t, flags=re.M | re.S):
        u = ' '.join(m.group(1).split())
        u = re.sub(r'\bcrate::', 'iyakki_agent_api::', u)
        u = re.sub(r'^crate\b', 'iyakki_agent_api', u)
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
    # supertraits and `self:` receivers decide whether a trait can be an object at all
    src = {}
    for f in {f for (f, _) in by}: src[f] = census.strip(open(ROOT + '/crates/iyakki-agent-api/src/' + f).read())
    ok, skipped = [], []
    for (f, t), ms in by.items():
        if allow is not None and t not in allow: continue
        head = re.search(r'pub trait %s\b([^{]*)\{' % t, src[f]).group(1)
        why = None
        if '<' in head or re.search(r':\s*[^+]*?(?<!Send)(?<!Sync)\s*(\+|$)', head.replace('Send', '').replace('Sync', '').replace('+', ' ').replace(':', ' ').strip() and head) and False: why = 'generic or supertrait'
        if 'Sync' not in head or 'Send' not in head: why = 'not `Send + Sync`'
        for m in ms:
            if not m['args'].lstrip().startswith('&self') and not re.match(r'^&self\b', m['args'].strip()): why = why or f"{m['fn']} has no `&self` receiver"
            if any(':' not in a and not re.match(r'^&?\s*(mut\s+)?self\b', a) for a in split_args(m['args'])): why = why or f"{m['fn']} has a signature the parser cannot read"
            if m['gen']: why = why or f"{m['fn']} is generic"
            if m['cls'] == 'H':
                h = handle_of(m['ret'])
                if not h or h[2] not in names: why = why or f"{m['fn']} returns a handle of a trait that is not served ({m['ret']})"
                elif h[2] == t and False: pass
            elif m['cls'] != 'S':
                if m['body']: m['mode'] = 'default'
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
                    ok.remove(item); skipped.append(item[:3] + (f"{m['fn']} returns a handle of {handle_of(m['ret'])[2]}, which is not generated",)); changed = True; break
    if '--list' in sys.argv:
        print('generated', len(ok), 'traits', sum(len(i[2]) for i in ok), 'methods')
        for f, t, ms, w in skipped: print('skipped', t, '-', w)
        return
    gen_remote(ok); gen_serve(ok)
    print('generated', len(ok), 'traits,', sum(len(i[2]) for i in ok), 'methods; skipped', len(skipped), 'traits')

def prelude(files):
    o = ['// GENERATED by dist/split/gen_rpc.py (F-SPLIT-03). Do not edit: change the script or the trait and run it again.', '#![allow(unused_imports, unused_variables, clippy::all, non_snake_case, dead_code)]', '']
    return o

def modname(f): return 'g_' + f.replace('.rs', '')

def gen_remote(ok):
    o = prelude(None)
    o += ['use std::sync::Arc;', 'use crate::{Lost, Rpc};', '']
    files = collections.OrderedDict()
    for f, t, ms, _ in ok: files.setdefault(f, []).append((t, ms))
    names = {t for (_, t, _, _) in ok}
    for f, items in files.items():
        o.append('mod %s {' % modname(f)); o.append('    use super::*;'); o.append('    use iyakki_agent_api::*;'); o.append('    use iyakki_agent_api::%s::*;' % f[:-3])
        for u in uses(ROOT + '/crates/iyakki-agent-api/src/' + f): o.append('    ' + u)
        for t, ms in items:
            o.append(f'    /// `{t}` over the connection (object `obj` of the core).')
            o.append(f'    pub struct Remote{t} {{ pub(crate) rpc: Rpc, pub(crate) obj: u64 }}')
            o.append(f'    impl Drop for Remote{t} {{ fn drop(&mut self) {{ self.rpc.release(self.obj); }} }}')
            o.append(f'    impl {t} for Remote{t} {{')
            for m in ms:
                if m.get('mode') == 'default': continue
                args = split_args(m['args'])
                params = [a for a in args if not re.match(r'^&?\s*(mut\s+)?self\b', a)]
                decl = []; names_ = []
                for p in params:
                    n, ty = p.split(':', 1); n = n.strip().lstrip('_') or 'a'; n = 'a_' + n if n in ('type', 'ref', 'match', 'fn', 'in') else n
                    decl.append(f'{n}: {ty.strip()}'); names_.append(n)
                ret = m['ret'] or '()'
                sig = f'        fn {m["fn"]}(&self{"".join(", " + d for d in decl)})' + (f' -> {m["ret"]}' if m['ret'] else '') + ' {'
                o.append(sig)
                argv = 'serde_json::json!([' + ', '.join(names_) + '])'
                if m.get('mode') == 'unserved':
                    o.append(f'            let _ = ({"".join(n + ", " for n in names_)});')
                    o.append(f'            <{ret} as Lost>::lost("this call is not served over the UI socket")')
                elif m['cls'] == 'H':
                    opt, kind, sub = handle_of(m['ret'])
                    call = f'self.rpc.call::<{"Option<u64>" if opt else "u64"}>("{t}", self.obj, "{m["fn"]}", {argv})'
                    mk = f'Remote{sub} {{ rpc: self.rpc.clone(), obj: id }}'
                    wrap = f'{kind}::new({mk})'
                    sub_mod = [fx for fx, its in files.items() if any(tt == sub for tt, _ in its)][0]
                    wrap = wrap.replace(f'Remote{sub}', f'super::{modname(sub_mod)}::Remote{sub}')
                    if opt:
                        o.append(f'            match {call} {{ Ok(Some(id)) => Some({wrap}), _ => None }}')
                    else:
                        # a lost core: an object that does not exist, so each call on it answers as a lost call does
                        lostwrap = wrap.replace('obj: id', 'obj: u64::MAX')
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
        for t, _ in items: o.append(f'        "{t}" => RemoteAny::{t}(Arc::new(Remote{t} {{ rpc: rpc.clone(), obj }})),')
    o.append('        _ => return None,'); o.append('    })'); o.append('}')
    o.append('/// One of the generated remote objects, by trait.'); o.append('pub enum RemoteAny {')
    for f, items in files.items():
        for t, _ in items: o.append(f'    {t}(Arc<dyn iyakki_agent_api::{f[:-3]}::{t}>),')
    o.append('}')
    open(OUT + '/gen_remote.rs', 'w').write('\n'.join(o) + '\n')

def gen_serve(ok):
    o = prelude(None)
    o += ['use std::sync::Arc;', 'use serde_json::Value;', 'use crate::{de, ser, Registry};', '']
    files = collections.OrderedDict()
    for f, t, ms, _ in ok: files.setdefault(f, []).append((t, ms))
    o.append('use iyakki_agent_api::*;')
    for f in files:
        o.append('mod %s { use super::*;' % modname(f)); o.append('    use iyakki_agent_api::%s::*;' % f[:-3])
        for u in uses(ROOT + '/crates/iyakki-agent-api/src/' + f): o.append('    ' + u)
        o.append('    pub(crate) trait Dummy {}')
        for t, ms in files[f]:
            o.append(f"    pub(crate) fn serve_{t}(t: &dyn {t}, reg: &Registry<'_>, method: &str, a: Value) -> Result<Value, String> {{")
            o.append('        match method {')
            for m in ms:
                if m.get('mode') in ('default', 'unserved'): continue
                args = split_args(m['args'])
                params = [a for a in args if not re.match(r'^&?\s*(mut\s+)?self\b', a)]
                tys, exprs, names_ = [], [], []
                for i, p in enumerate(params):
                    n, ty = p.split(':', 1); ow, ex = owned(ty.strip())
                    names_.append(f'x{i}'); tys.append(ow); exprs.append(ex.replace('x', f'x{i}', 1) if ex.startswith('x') else ex.replace('x', f'x{i}'))
                o.append(f'            "{m["fn"]}" => {{')
                if params:
                    pat = '(' + ''.join(n + ', ' for n in names_) + ')'
                    ty = '(' + ''.join(t_ + ', ' for t_ in tys) + ')'
                    o.append(f'                let {pat}: {ty} = de(a)?;')
                call = f't.{m["fn"]}(' + ', '.join(exprs) + ')'
                if m['cls'] == 'H':
                    opt, kind, sub = handle_of(m['ret'])
                    conv = 'h' if kind == 'Arc' else 'Arc::from(h)'
                    if opt: o.append(f'                ser(&{call}.map(|h| reg.add(Obj::{sub}({conv}))))')
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
        for t, _ in items: o.append(f'    {t}(Arc<dyn iyakki_agent_api::{f[:-3]}::{t}>),')
    o.append('}')
    o.append('impl Obj {'); o.append('    pub fn iface(&self) -> &\'static str {'); o.append('        match self {')
    for f, items in files.items():
        for t, _ in items: o.append(f'            Obj::{t}(_) => "{t}",')
    o.append('        }'); o.append('    }')
    o.append("    pub(crate) fn serve(&self, reg: &Registry<'_>, method: &str, a: Value) -> Result<Value, String> {"); o.append('        match self {')
    for f, items in files.items():
        for t, _ in items: o.append(f'            Obj::{t}(t) => {modname(f)}::serve_{t}(t.as_ref(), reg, method, a),')
    o.append('        }'); o.append('    }'); o.append('}')
    open(OUT + '/gen_serve.rs', 'w').write('\n'.join(o) + '\n')

if __name__ == "__main__":
    main()

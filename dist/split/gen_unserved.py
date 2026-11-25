#!/usr/bin/env python3
"""gen_unserved.py: crates/iyakki-ui-rpc/src/gen_unserved.rs, an `Unserved` object of every facet trait the remote `ServicesHandle` answers with
when the core does not serve that trait over the socket yet (D-SPLIT-03-HANDLE: "a quiet value of that facet"). Every method answers the quiet value
of its return type (`Lost`: `Err("not served over the UI socket")`, `None`, an empty list, 0, false); a method with a default body in the trait keeps it.
The traits are the handles of the quiet methods of `ServicesHandle` and, until nothing new appears, the handles their methods return.
Usage: dist/split/gen_unserved.py [--check]"""
import re, os, sys, importlib.util
HERE = os.path.dirname(os.path.abspath(__file__))
def load(n):
    s = importlib.util.spec_from_file_location(n, os.path.join(HERE, n + '.py')); m = importlib.util.module_from_spec(s); s.loader.exec_module(m); return m
gs = load('gen_services'); g = gs.g; census = g.census; ROOT = census.ROOT
OUT = ROOT + '/crates/iyakki-ui-rpc/src/gen_unserved.rs'
# traits of other crates, written by hand in unserved.rs
# traits of other crates: (file, the crate, the module path inside it). The agent-api ones are found by the census itself.
EXTERNAL = {
    'Tools': ('crates/iyakki-shared/src/agent.rs', 'iyakki_shared', 'agent'), 'ChatBackend': ('crates/iyakki-chat-model/src/lib.rs', 'iyakki_shared', 'chat'),
    'CodeVault': ('crates/iyakki-shared/src/authcodes.rs', 'iyakki_shared', 'authcodes'), 'Synth': ('crates/iyakki-story-model/src/lib.rs', 'iyakki_shared', 'story'),
    'Speaker': ('crates/iyakki-voice-model/src/voiceassist.rs', 'iyakki_shared', 'voiceassist'), 'TurnLog': ('crates/iyakki-voice-model/src/voiceassist.rs', 'iyakki_shared', 'voiceassist'),
    'LessonHost': ('crates/iyakki-shared/src/studylesson.rs', 'iyakki_shared', 'studylesson'), 'Kube': ('crates/iyakki-shared/src/modelhost.rs', 'iyakki_shared', 'modelhost'),
    'Runner': ('crates/iyakki-k8s-model/src/wsexec.rs', 'iyakki_shared', 'wsexec'), 'Kubectl': ('crates/iyakki-sysinfo-model/src/kubectl.rs', 'iyakki_shared', 'kubectl'),
    'Mic': ('crates/iyakki-audiolink/src/lib.rs', 'iyakki_shared', 'wakelisten'), 'DashHost': ('crates/iyakki-dash/src/tools.rs', 'iyakki_dash', 'tools'),
    'Upstream': ('crates/iyakki-proxy/src/lib.rs', 'iyakki_proxy', ''), 'RemoteSource': ('crates/iyakki-tile-api/src/remote.rs', 'iyakki_tile_api', 'remote'),
}
census.EXTRA_FILES = sorted({ROOT + '/' + v[0] for v in EXTERNAL.values()})
# written by hand in unserved.rs (a trait with a quiet answer that cannot be written from the signature, or one that exists already)
HAND = {'CueOut', 'StoreFacets'}

DEPS = {'std', 'core', 'alloc', 'serde', 'serde_json', 'zeroize'} | {l.split('=')[0].strip().replace('-', '_') for l in open(ROOT + '/crates/iyakki-ui-rpc/Cargo.toml') if '=' in l and not l.startswith(('version', 'edition', 'license', 'description'))}

def uses(path, crate):
    t = census.strip(open(path).read()); out = []
    for m in re.finditer(r'^use\s+([^;]*?);', t, flags=re.M | re.S):
        u = ' '.join(m.group(1).split())
        u = re.sub(r'\bcrate::', crate + '::', u); u = re.sub(r'^crate\b', crate, u)
        if u.startswith('super') or u.startswith('self'): continue
        if u.split('::')[0] not in DEPS: continue
        out.append('use ' + u + ';')
    return out

def build():
    res = census.build()
    by = {}
    for r in res: by.setdefault(r['trait'], []).append(r)
    files = {r['trait']: r['file'] for r in res}
    ms, names = gs.parse()
    allow = set(re.findall(r'pub struct Remote(\w+) \{', open(ROOT + '/crates/iyakki-ui-rpc/src/gen_remote.rs').read()))
    need = []
    def want(ret):
        if re.match(r'^(Option|Result)<', (ret or '').strip()): return None  # their quiet value is None / Err
        m = re.search(r'(?:Arc|Box)<(?:Mutex<)?dyn\s+([\w:]+)', ret or '')
        return m.group(1).split('::')[-1] if m else None
    for m in ms:
        if gs.kind(m, names, allow) == 'quiet':
            t = want(m['ret'])
            if t and t not in need: need.append(t)
    # traits whose accessor is served by hand now (services-manual.txt) but whose quiet object other generated remotes still hand out
    for t in ('Kubectl', 'Upstream', 'Synth', 'Runner'):
        if t not in need: need.append(t)
    i = 0
    while i < len(need):
        for r in by.get(need[i], []):
            if r['body']: continue
            t = want(r['ret'])
            if t and t not in need: need.append(t)
        i += 1
    need = [t for t in need if t != 'Any']
    # a supertrait of a needed trait needs its impl too (no `Lost`: nothing hands it out alone)
    supers = []
    for t in need:
        r0 = by.get(t)
        if not r0: continue
        head = re.search(r'pub trait %s\b([^{]*)\{' % t, census.strip(open(r0[0]['path']).read())).group(1)
        for sname in re.findall(r'\b([A-Z]\w*)\b', head.split(':', 1)[1] if ':' in head else ''):
            if sname not in ('Send', 'Sync') and sname in by and sname not in need and sname not in supers: supers.append(sname)
    hand = [t for t in need if t in HAND]
    gen = [t for t in need if t not in HAND] + supers
    o = ['// GENERATED by dist/split/gen_unserved.py (F-SPLIT-03, D-SPLIT-03-HANDLE). Do not edit: change the script or the trait and run it again.',
         '#![allow(unused_imports, unused_variables, clippy::all, non_snake_case, dead_code)]', '', 'use std::sync::Arc;', 'use crate::Lost;', 'use crate::unserved::Unserved;', '']
    bypath = {}
    for t in gen: bypath.setdefault(by[t][0]['path'], []).append(t)
    for path, ts in bypath.items():
        rel = os.path.relpath(path, ROOT); stem = os.path.basename(path)[:-3]
        ext = [v for v in EXTERNAL.values() if v[0] == rel]
        crate, mod = (ext[0][1], ext[0][2]) if ext else ('iyakki_agent_api', stem)
        uname = stem if stem != 'lib' else os.path.basename(os.path.dirname(os.path.dirname(path))).replace('-', '_')  # two crates can both have a lib.rs
        o += ['mod u_%s_%s {' % (crate, uname), '    use super::*;', '    use iyakki_agent_api::*;']
        if mod: o.append(f'    use {crate}::{mod}::*;')
        else: o.append(f'    use {crate}::*;')
        for u in uses(path, crate): o.append('    ' + u)
        for t in ts:
            head = re.search(r'pub trait %s\b([^{]*)\{' % t, census.strip(open(path).read())).group(1)
            o.append(f'    impl {t} for Unserved {{')
            for r in by[t]:
                if r['body']: continue
                args = r['args']; ret = r['ret']
                rr = re.sub(r'\bcrate::', crate + '::', ret); aa = re.sub(r'\bcrate::', crate + '::', args)
                sig = f'        fn {r["fn"]}{r["gen"]}({aa})' + (f' -> {rr}' if rr else '') + ' {'
                o.append(sig)
                if rr.startswith('&'):
                    inner = rr[1:].strip()
                    if inner == 'str': o.append('            ""')
                    elif inner in ('Path', 'std::path::Path'): o.append('            std::path::Path::new("")')
                    elif inner.startswith('['): o.append('            &[]')
                    elif inner.startswith('dyn '): o.append('            unreachable!("a borrowed trait object has no quiet value")')
                    else: o.append(f'            {{ static V: std::sync::OnceLock<{inner}> = std::sync::OnceLock::new(); V.get_or_init(|| <{inner} as Lost>::lost(super::NOT_SERVED)) }}')
                else:
                    o.append(f'            <{rr or "()"} as Lost>::lost(super::NOT_SERVED)')
                o.append('        }')
            o.append('    }')
            if t not in supers:
                o.append(f'    impl Lost for Arc<dyn {t}> {{ fn lost(_: &str) -> Self {{ Arc::new(Unserved) }} }}')
                o.append(f'    impl Lost for Box<dyn {t}> {{ fn lost(_: &str) -> Self {{ Box::new(Unserved) }} }}')
                if True:
                    o.append(f'    impl Lost for Arc<dyn {t} + Send + Sync> {{ fn lost(_: &str) -> Self {{ Arc::new(Unserved) }} }}')
        o.append('}')
    o.append('pub(crate) const NOT_SERVED: &str = "%s";' % gs.NOT_SERVED)
    return '\n'.join(o) + '\n', hand

if __name__ == '__main__':
    txt, hand = build()
    if '--check' in sys.argv:
        if open(OUT).read() != txt: print('gen_unserved.rs is stale: run dist/split/gen_unserved.py'); sys.exit(1)
        print('fresh'); sys.exit(0)
    open(OUT, 'w').write(txt); print('wrote', OUT, '; by hand:', hand)

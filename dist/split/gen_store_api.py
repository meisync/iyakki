#!/usr/bin/env python3
"""gen_store_api.py: one trait per concrete member store (F-SPLIT-03, UI-RPC-DESIGN.md section 1, point 2).

Reads the public `&self` methods of the inherent impl blocks of each store and writes crates/iyakki-agent-api/src/member_stores.rs: a trait
`XApi` with those methods and `impl XApi for X` delegating to the inherent method (the impl may live in the trait crate because it depends on
the crate of the type). `&mut self`, generic, `&'a`-returning, closure-taking and `impl Trait` methods stay inherent and are counted in the header.
Skips come from dist/split/store-api-skip.txt (`Trait::method`, one per line) for what the compiler refused.
"""
import re, sys, os
ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
STORES = [  # (source file, module path of the type, struct, trait)
    ("crates/iyakki-shared/src/inbox.rs", "iyakki_shared::inbox", "Inbox", "InboxApi"),
    ("crates/iyakki-shared/src/calendar.rs", "iyakki_shared::calendar", "CalendarStore", "CalendarApi"),
    ("crates/iyakki-shared/src/study.rs", "iyakki_shared::study", "StudyStore", "StudyApi"),
    ("crates/iyakki-shared/src/vault.rs", "iyakki_shared::vault", "Secrets", "SecretsApi"),
    ("crates/iyakki-shared/src/devwatch.rs", "iyakki_shared::devwatch", "DeviceWatch", "DevWatchApi"),
    ("crates/iyakki-shared/src/browserconsent.rs", "iyakki_shared::browserconsent", "BrowserState", "BrowserStateApi"),
    ("crates/iyakki-shared/src/browserconsent.rs", "iyakki_shared::browserconsent", "ConsentStore", "ConsentApi"),
    ("crates/iyakki-shared/src/browserjobs.rs", "iyakki_shared::browserjobs", "BrowserJobs", "BrowserJobsApi"),
    ("crates/iyakki-shared/src/screencap.rs", "iyakki_shared::screencap", "ScreenCapture", "ScreenCaptureApi"),
    ("crates/iyakki-shared/src/camtools.rs", "iyakki_shared::camtools", "CameraHub", "CameraHubApi"),
    ("crates/iyakki-shared/src/devtools.rs", "iyakki_shared::devtools", "DevTools", "DevToolsApi"),
    ("crates/iyakki-shared/src/secretreq.rs", "iyakki_shared::secretreq", "SecretRequests", "SecretRequestsApi"),
    ("crates/iyakki-shared/src/usagelog.rs", "iyakki_shared::usagelog", "UsageLog", "UsageLogApi"),
    ("crates/iyakki-shared/src/extagent/mod.rs", "iyakki_shared::extagent", "StatusCache", "ExtStatusApi"),
    ("crates/iyakki-memx/src/kb.rs", "iyakki_memx::kb", "KnowledgeBase", "KnowledgeBaseApi"),
    ("crates/iyakki-accounts/src/lib.rs", "iyakki_accounts", "UserStore", "AccountsApi"),
    ("crates/iyakki-accounts/src/lib.rs", "iyakki_accounts", "AuditLog", "AuditApi"),
]
# more inherent impl blocks of the same store in other files: (file, module of the types they name)
EXTRA = {"StudyApi": [("crates/iyakki-shared/src/studylesson.rs", "iyakki_shared::studylesson")]}
# stores the services handle already hands out as `Arc<dyn XApi>` (the shell holds the trait, not the type): their Via forwards to the trait object
MIGRATED = {'KnowledgeBaseApi', 'ExtStatusApi', 'BrowserStateApi', 'ConsentApi', 'BrowserJobsApi', 'DevToolsApi', 'CameraHubApi', 'ScreenCaptureApi', 'InboxApi', 'UsageLogApi', 'SecretRequestsApi', 'DevWatchApi', 'CalendarApi', 'StudyApi'}
VIA = {'InboxApi': 'inbox', 'CalendarApi': 'calendar', 'StudyApi': 'study', 'SecretsApi': 'secrets', 'DevWatchApi': 'devwatch', 'BrowserStateApi': 'browser', 'ConsentApi': 'browser_consent', 'BrowserJobsApi': 'browser_jobs',
       'ScreenCaptureApi': 'screen', 'CameraHubApi': 'camera', 'DevToolsApi': 'devtools', 'SecretRequestsApi': 'secret_requests', 'UsageLogApi': 'usage', 'ExtStatusApi': 'ext_status', 'KnowledgeBaseApi': 'kb'}
# methods written by hand (a closure cannot cross the socket; D-SPLIT-03-SINK): trait -> [(signature without `fn`, name, parameter names)]; the remote end ignores them
AC = 'iyakki_shared::extagentcfg::AgentsCfg'
SINKS = {'BrowserStateApi': [('set_audit_sink(&self, sink: Arc<dyn Fn(&str, &str, &str) + Send + Sync>)', 'set_audit', ['sink'], '')],
         # the lookups that spawn a thread of their own took `self: &Arc<Self>`: the trait is implemented for the Arc, and the completion callback is dropped (nobody passed one)
         'ExtStatusApi': [(f'refresh(&self, cfg: {AC})', 'refresh', ['cfg'], ''), (f'reset(&self, cfg: {AC})', 'reset', ['cfg'], ''),
                          (f'recheck(&self, cfg: {AC}, id: &str)', 'recheck', ['cfg', 'id'], ''),
                          (f'recheck_after(&self, cfg: {AC}, id: &str, min: Duration)', 'recheck_after', ['cfg', 'id', 'min'], ''),
                          (f'refresh_one(&self, cfg: {AC}, id: &str)', 'refresh_one', ['cfg', 'id'], ', None')]}
# the type the trait is implemented for when it is not the struct itself
IMPL_FOR = {'AccountsApi': 'std::sync::Arc<std::sync::Mutex<iyakki_accounts::UserStore>>', 'ExtStatusApi': 'std::sync::Arc<iyakki_shared::extagent::StatusCache>', 'KnowledgeBaseApi': 'std::sync::Arc<std::sync::Mutex<iyakki_memx::kb::KnowledgeBase>>'}
# stores the services keep behind a lock: `&mut self` methods become `&self` trait methods, each takes the lock (a poisoned lock is recovered, like the callers did)
LOCKED = {'KnowledgeBaseApi', 'AccountsApi'}
skip = set()
p = os.path.join(ROOT, "dist/split/store-api-skip.txt")
if os.path.exists(p):
    skip = {l.strip() for l in open(p) if l.strip() and not l.startswith("#")}

def impl_blocks(src, struct):
    for m in re.finditer(r'\nimpl(?:<[^>]*>)?\s+' + struct + r'(?:<[^>]*>)?\s*\{', src):
        i = m.end(); d = 1
        while d and i < len(src):
            c = src[i]
            d += (c == '{') - (c == '}')
            i += 1
        yield src[m.end():i - 1]

def methods(body):
    # top-level `pub fn` of the block (depth 0 of the block)
    out = []; i = 0; depth = 0; n = len(body)
    while i < n:
        c = body[i]
        if c == '{': depth += 1
        elif c == '}': depth -= 1
        elif depth == 0 and body.startswith("pub fn ", i) and (i == 0 or body[i-1] in " \n\t"):
            j = i; par = 0
            while j < n and not (body[j] in '{;' and par == 0):
                par += (body[j] == '(') - (body[j] == ')') ; j += 1
            sig = body[i + 7:j].strip()
            out.append(sig)
            i = j; continue
        i += 1
    return out

def split_sig(sig):
    m = re.match(r'(\w+)\s*(<[^(]*>)?\s*\(', sig)
    if not m: return None
    name, gen = m.group(1), m.group(2)
    k = m.end(); par = 1; a = k
    while par:
        ch = sig[k]
        par += (ch == '(') - (ch == ')')
        k += 1
    args = sig[a:k - 1]; rest = sig[k:].strip()
    ret = rest[2:].strip() if rest.startswith('->') else ''
    ret = re.sub(r'\s+where\b.*$', '', ret, flags=re.S).strip()
    return name, gen, args, ret

def split_args(args):
    out = []; d = 0; cur = ''
    for ch in args:
        if ch in '<([': d += 1
        if ch in '>)]' : d -= 1
        if ch == ',' and d == 0:
            out.append(cur); cur = ''
        else: cur += ch
    if cur.strip(): out.append(cur)
    return [re.sub(r'\s+', ' ', a.strip()) for a in out if a.strip()]

FIXED = {'Role': 'iyakki_sec::policy::Role', 'PolicyOverride': 'iyakki_sec::policy::PolicyOverride', 'ModelInfo': 'iyakki_extagentx::ModelInfo', 'Status': 'iyakki_extagentx::Status', 'Zone': 'iyakki_clock::Zone', 'Duration': 'std::time::Duration', 'Zeroizing': 'zeroize::Zeroizing', 'CalendarStore': 'iyakki_shared::calendar::CalendarStore',
         'BTreeMap': 'std::collections::BTreeMap', 'HashMap': 'std::collections::HashMap', 'HashSet': 'std::collections::HashSet', 'BTreeSet': 'std::collections::BTreeSet',
         'Path': 'std::path::Path', 'PathBuf': 'std::path::PathBuf', 'Arc': 'std::sync::Arc', 'Value': 'serde_json::Value'}
KEEP = {'String', 'Vec', 'Option', 'Result', 'Box', 'Self'}

def qualifier(src, mod, extra=()):
    m = dict(FIXED)
    for sr, mo in [(src, mod)] + list(extra):
      for d in re.finditer(r'\bpub (?:struct|enum|type|trait) ([A-Z]\w*)', sr):
        (m.__setitem__ if sr is src else m.setdefault)(d.group(1), mo + '::' + d.group(1))
      for d in re.finditer(r'\bpub use ([\w:]+)::\{?([^;]*?)\}?;', sr):
        for n in re.findall(r'\b([A-Z]\w*)\b(?!\s*::)', d.group(2)):
            m.setdefault(n, d.group(1) + '::' + n)
    def q(text):
        return re.sub(r'(?<![\w:])([A-Z]\w*)\b(?!\s*::)', lambda x: m.get(x.group(1), x.group(1)) if x.group(1) not in KEEP else x.group(1), text)
    return q

hdr = ['//! GENERATED by dist/split/gen_store_api.py: one trait per concrete member store of the services (F-SPLIT-03), the same public `&self` methods as the type,',
       '//! each implemented for the type by delegation. A UI that holds `Arc<dyn XApi>` can run over the socket (`iyakki-ui-rpc` generates the remote end).',
       '#![allow(clippy::all, unused_imports)]', '#![allow(dead_code)]', '', 'use std::collections::{BTreeMap, HashMap, HashSet};', 'use std::path::{Path, PathBuf};', 'use std::sync::Arc;', 'use serde_json::Value;', 'use crate::ServicesHandle;', 'use std::time::Duration;', 'use zeroize::Zeroizing;', 'use iyakki_shared::calendar::CalendarStore;', '']
body = []; stats = []
for f, mod, struct, trait in STORES:
    src = open(os.path.join(ROOT, f)).read()
    if not mod.startswith("iyakki_agent_api"):
        pass
    ms = []; kept = 0; left = 0
    extra = [(open(os.path.join(ROOT, ef)).read(), em) for ef, em in EXTRA.get(trait, [])]
    for blk in [b for sr in [src] + [e[0] for e in extra] for b in impl_blocks(sr, struct)]:
        for sig in methods(blk):
            s = split_sig(sig)
            if not s: left += 1; continue
            name, gen, args, ret = s
            parts = split_args(args)
            ok = parts and (parts[0] == '&self' or (trait in LOCKED and parts[0] == '&mut self')) and not gen and "impl " not in sig and "Fn" not in sig and "&'" not in ret and "&mut" not in ' '.join(parts[1:]) and 'crate::' not in sig and 'AuditSink' not in sig and '&[&' not in sig and '&[(&' not in sig and not re.search(r'CalendarStore|MutexGuard|LocalVault', sig) and '&' not in ret and 'Self' not in ret and 'Self' not in args.replace('&self', '') and f"{trait}::{name}" not in skip
            if not ok: left += 1; continue
            pn = []
            for pa in parts[1:]:
                nm = re.sub(r'^mut\s+', '', pa.split(':')[0].strip()); pn.append(nm)
            ms.append((name, ['&self'] + parts[1:], ret, pn, parts[0] == '&mut self')); kept += 1
    seen = set(); ms2 = []; muts = {}
    for m in ms:
        if m[0] in seen: continue
        seen.add(m[0]); ms2.append(m[:4]); muts[m[0]] = m[4]
    q = qualifier(src, mod, extra)
    ms2 = [(n, [parts[0]] + [q(x) for x in parts[1:]], q(r), pn) for n, parts, r, pn in ms2]
    stats.append((trait, len(ms2), left))
    body.append(f'pub mod {trait.lower()}_mod {{')
    body.append('    use super::*;')
    body.append(f'/// `{struct}` of `{mod}` ({len(ms2)} methods; {left} stay inherent: `&mut self`, generic, closures or borrowed answers).')
    body.append(f'pub trait {trait}: Send + Sync {{')
    for name, parts, ret, pn in ms2:
        body.append(f'    fn {name}({", ".join(parts)}){" -> " + ret if ret else ""};')
    for sg, nm, pnames, tail in SINKS.get(trait, []):
        body.append(f'    fn {sg};')
    body.append('}')
    body.append(f'impl {trait} for {IMPL_FOR.get(trait, mod + "::" + struct)} {{')
    for sg, nm, pnames, tail in SINKS.get(trait, []):
        body.append(f'    fn {sg} {{')
        body.append(f'        {mod}::{struct}::{nm}({", ".join(["self"] + pnames)}{tail})')
        body.append('    }')
    for name, parts, ret, pn in ms2:
        body.append(f'    fn {name}({", ".join(parts)}){" -> " + ret if ret else ""} {{')
        if trait in LOCKED:
            body.append(f'        let {"mut " if muts[name] else ""}g = self.lock().unwrap_or_else(|e| e.into_inner());')
            body.append(f'        {mod}::{struct}::{name}({", ".join(["&mut *g" if muts[name] else "&*g"] + pn)})')
        else:
            body.append(f'        {mod}::{struct}::{name}({", ".join(["self"] + pn)})')
        body.append('    }')
    body.append('}')
    if trait in VIA:
        # the member as the services handle holds it: the same methods, reached through the accessor, so the core can serve a root over its real store
        via = trait[:-3] + 'Via'
        body.append(f'/// `{trait}` over the `{VIA[trait]}()` member of a services handle.')
        body.append(f'pub struct {via}(pub Arc<dyn ServicesHandle>);')
        body.append(f'impl {trait} for {via} {{')
        for sg, nm, pnames, tail in SINKS.get(trait, []):
            body.append(f'    fn {sg} {{')
            body.append(f'        self.0.{VIA[trait]}().{sg.split("(")[0]}({", ".join(pnames)})')
            body.append('    }')
        for name, parts, ret, pn in ms2:
            body.append(f'    fn {name}({", ".join(parts)}){" -> " + ret if ret else ""} {{')
            if trait in MIGRATED:
                body.append(f'        self.0.{VIA[trait]}().{name}({", ".join(pn)})')
            else:
                body.append(f'        {mod}::{struct}::{name}({", ".join(["self.0." + VIA[trait] + "()"] + pn)})')
            body.append('    }')
        body.append('}')
    body.append('}')
    body.append(f'pub use {trait.lower()}_mod::{trait};')
    if trait in VIA:
        body.append(f'pub use {trait.lower()}_mod::{trait[:-3]}Via;')
    body.append('')
out = os.path.join(ROOT, "crates/iyakki-agent-api/src/member_stores.rs")
open(out, "w").write('\n'.join(hdr + body))
for t, k, l in stats: print(f"{t}: {k} methods, {l} inherent")
print("total", sum(k for _, k, _ in stats))

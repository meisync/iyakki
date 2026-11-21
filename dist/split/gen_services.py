#!/usr/bin/env python3
"""gen_services.py: `RemoteServicesHandle` as a `ServicesHandle` (F-SPLIT-03 part 1, D-SPLIT-03-HANDLE, UI-RPC-DESIGN.md sections 4 and 5).

Reads the trait `ServicesHandle` (same parser as the census) and writes
  crates/iyakki-ui-rpc/src/gen_services.rs   the client `impl ServicesHandle for RemoteServicesHandle` and the core's dispatcher `serve_services`
Per method (the decision of D-SPLIT-03-HANDLE, nothing made up):
  wire     plain data both ways, and the handle accessors whose trait is generated (a handle is a remote object, a few are roots the core already serves);
  audit    the audit callback is left out of the wire: the core passes its own (`ServicesRoot::audit`);
  kept     answered by `RemoteServicesHandle::k_<name>` written by hand in handle.rs (borrowed answers, closures that change a value, `&'static str` in an answer);
  noop     `set_streamer`, `set_chat_streamer` (the core owns its streamers);
  quiet    everything else: the quiet value of the answer (`Lost`), or for a facet whose trait is not generated yet the `Unserved` object of that trait.
A method with a body in the trait that is not named in OVERRIDE keeps its default. `--check` writes nothing and fails when the file is stale.
Usage: dist/split/gen_services.py [--check]"""
import re, os, sys, importlib.util
HERE = os.path.dirname(os.path.abspath(__file__))
def load(n):
    s = importlib.util.spec_from_file_location(n, os.path.join(HERE, n + '.py')); m = importlib.util.module_from_spec(s); s.loader.exec_module(m); return m
g = load('gen_rpc'); census = g.census; ROOT = census.ROOT
OUT = ROOT + '/crates/iyakki-ui-rpc/src/gen_services.rs'
NOT_SERVED = 'not served over the UI socket'

# accessors the core already serves as a root object (object 0 of that trait): the inherent accessor of RemoteServicesHandle answers them
ROOT_ACCESSORS = {'settings_api': 'SettingsApi', 'inbox': 'InboxApi', 'calendar': 'CalendarApi', 'study': 'StudyApi', 'secrets_api': 'SecretsApi', 'kb': 'KnowledgeBaseApi',
                  'devwatch': 'DevWatchApi', 'browser': 'BrowserStateApi', 'browser_consent': 'ConsentApi', 'browser_jobs': 'BrowserJobsApi', 'screen': 'ScreenCaptureApi',
                  'devtools': 'DevToolsApi', 'ext_status': 'ExtStatusApi', 'secret_requests': 'SecretRequestsApi', 'usage': 'UsageLogApi'}
KEPT = {'dir', 'mcp', 'secrets', 'cloud_stream_prompt_rule', 'fleet_prompt_rule', 'update_sound_with', 'update_system_sounds_with', 'is_real',
        'router_plan_rows'}
NOOP = {'set_streamer', 'set_chat_streamer'}
# accessors written by hand (served.rs, D-SPLIT-03-SERVED): the client calls `RemoteServicesHandle::m_<name>`, the core's end is `served::serve_manual`
MANUAL = set(l.split('#')[0].strip() for l in open(os.path.join(HERE, 'services-manual.txt'))) - {''} if os.path.exists(os.path.join(HERE, 'services-manual.txt')) else set()
AUDIT = {'domains_login', 'install_bt_remote', 'msg_mirror_tick', 'rag_refresh_tick'}
# what the owner's secret rule keeps off the wire (D-SPLIT-03-SECRETS): the answer would be a plain secret
SECRET = {'agent_vault_token'}
# plain methods whose types cannot cross as they are (no serde on a type): the quiet value, listed so the work list is visible
UNSERVED_S = set(l.split('#')[0].strip() for l in open(os.path.join(HERE, 'services-unserved.txt'))) - {''} if os.path.exists(os.path.join(HERE, 'services-unserved.txt')) else set()
OVERRIDE = {'secrets_api'}  # a method with a body that the remote end answers itself

def parse():
    res = census.build()
    ms = [r for r in res if r['trait'] == 'ServicesHandle']
    names = {r['trait'] for r in res}
    return ms, names

def fix(s): return s.replace('crate::', 'iyakki_agent_api::')

def params(m):
    out = []
    for a in g.split_args(m['args']):
        if re.match(r'^&?\s*(mut\s+)?self\b', a): continue
        n, ty = a.split(':', 1); n = n.strip().lstrip('_') or 'a'; n = re.sub(r'^mut\s+', '', n)
        out.append((n, ty.strip()))
    return out

def owned(ty):
    """(owned type on the wire, expression turning the decoded `x` into the argument)"""
    if re.match(r'^&\s*\[\s*&\s*str\s*\]$', ty): return 'Vec<String>', '&x.iter().map(String::as_str).collect::<Vec<&str>>()'
    return g.owned(ty)

def is_audit(t): return 'AuditFn' in t or re.search(r'dyn Fn\(&str, &str\)', t) is not None

# objects and closures the UI would supply, which the core replaces with its own (D-SPLIT-03-ENV): the wire carries the data arguments only.
# (type regex, the core's expression: `env` is the root's CoreEnv, `t` the real services, `root` the ServicesRoot)
ENV_ARGS = [
    (r'^(crate::|iyakki_agent_api::)?SftpLogin$', 'env.sftp_login()'),
    (r'^(crate::|iyakki_agent_api::)?SftpLookup$', 'env.sftp_lookup()'),
    (r'^Arc<dyn iyakki_core_api::Os>$', 'env.os.clone()'),
    (r'^Arc<dyn Fleet>$', 'env.os.fleet()'),
    (r'^Arc<dyn iyakki_core_api::router::Router>$', 'env.os.router()'),
    (r'^Arc<dyn CueOut>$', 't.cue_out()'),
]
def env_expr(t):
    for rx, ex in ENV_ARGS:
        if re.match(rx, t.strip()): return ex
    return None
def dropped(t): return is_audit(t) or env_expr(t) is not None
# methods of D-SPLIT-03-ENV: every argument is data, an audit callback or an object the core owns itself
ENV_METHODS = {'sftp_server', 'router_panel', 'fleet_view', 'fleet_act', 'preview_cue'}

def kind(m, names, allow_h):
    f = m['fn']
    if f in KEPT: return 'kept'
    if f in MANUAL: return 'manual'
    if f in NOOP: return 'noop'
    if f in AUDIT: return 'audit'
    if f in ENV_METHODS:
        ps_ = params(m)
        assert all(dropped(t) or g.owned(t) or owned(t) for _, t in ps_), f
        return 'env'
    if m['body'] and f not in OVERRIDE: return 'default'
    # an accessor whose only argument besides data is the audit callback: the callback stays on the core's side (D-SPLIT-03-HANDLE (b))
    if m['cls'] == 'E' and f not in ROOT_ACCESSORS:
        h = g.handle_of(m['ret'])
        ps_ = params(m)
        if h and h[2] in allow_h and not h[0] and h[1] == 'Arc' and any(is_audit(t) for _, t in ps_) and all(is_audit(t) or g.owned(t) for _, t in ps_) and not any(re.search(r'SftpLogin|SftpLookup|SharedOpen|TaskList|AuditLine', t) for _, t in ps_) and not any(('dyn Fn' in t and not is_audit(t)) or 'Arc<dyn' in t and not is_audit(t) for _, t in ps_):
            return 'handle'
    if f in ROOT_ACCESSORS: return 'root'
    if m['cls'] == 'H':
        h = g.handle_of(m['ret'])
        if h and h[2] in allow_h and not h[0] and h[1] == 'Arc': return 'handle'
        return 'quiet'
    if m['cls'] == 'S' and f not in UNSERVED_S and f not in SECRET: return 'wire'
    return 'quiet'

def build():
    ms, names = parse()
    # the traits gen_rpc.py really generated (a name in rpc-traits.txt that was skipped has no `Remote*`)
    allow = set(re.findall(r'pub struct Remote(\w+) \{', open(ROOT + '/crates/iyakki-ui-rpc/src/gen_remote.rs').read()))
    o = ['// GENERATED by dist/split/gen_services.py (F-SPLIT-03, D-SPLIT-03-HANDLE). Do not edit: change the script or the trait and run it again.',
         '#![allow(unused_imports, unused_variables, clippy::all, dead_code)]', '',
         'use iyakki_agent_api::*;', 'use iyakki_agent_api::facets_h::*;',
         'use serde_json::json;', 'use crate::{de, ser, Lost, Obj, Registry, RemoteServicesHandle, ServicesRoot};', 'use crate::gen_remote::*;', 'use crate::unserved::Unserved;', '']
    for u in g.uses(ROOT + '/crates/iyakki-agent-api/src/facets_h.rs'):
        if 'recording' in u: continue
        o.append(u)
    o += ['', 'const NOT_SERVED: &str = "%s";' % NOT_SERVED, '', 'impl ServicesHandle for RemoteServicesHandle {']
    serve = []
    for m in ms:
        f = m['fn']; k = kind(m, names, allow)
        if k == 'default': continue
        ps = params(m); ret = fix(m['ret']) if m['ret'] else ''
        decl = ''.join(f', {n}: {fix(t)}' for n, t in ps)
        recv = '&self'
        if f == 'as_any':
            o += ['    fn as_any(&self) -> &dyn Any {', '        self', '    }']; continue
        if f == 'as_any_arc':
            o += ['    fn as_any_arc(self: Arc<Self>) -> Arc<dyn Any + Send + Sync> {', '        self', '    }']; continue
        sig = f'    fn {f}({recv}{decl})' + (f' -> {ret}' if ret else '') + ' {'
        o.append(sig)
        keep_names = [n for n, t in ps]
        if k == 'kept':
            o.append(f'        self.k_{f}({", ".join(keep_names)})')
        elif k == 'manual':
            o.append(f'        self.m_{f}({", ".join(keep_names)})')
        elif k == 'noop':
            o.append(f'        let _ = ({"".join(n + ", " for n in keep_names)});')
        elif k == 'root':
            o.append(f'        RemoteServicesHandle::{f}(self)')
        elif k in ('wire', 'audit', 'handle', 'env'):
            envm = k == 'env'
            if envm:
                hret = g.handle_of(m['ret'])
                k = 'handle' if (hret and hret[2] in allow and not hret[0] and hret[1] == 'Arc') else 'wire'
            def drop(t): return (k == 'audit' and ('AuditFn' in t or 'dyn Fn' in t)) or (k == 'handle' and is_audit(t)) or (envm and dropped(t))
            sent = [(n, t) for n, t in ps if not drop(t)]
            argv = 'json!([' + ', '.join(n for n, t in sent) + '])'
            r0 = ret or '()'
            if k == 'handle':
                sub = g.handle_of(m['ret'])[2]
                o.append(f'        match self.rpc.call::<u64>("ServicesHandle", 0, "{f}", {argv}) {{ Ok(id) => Arc::new(Remote{sub}::new(self.rpc.clone(), id)), Err(_) => Arc::new(Remote{sub}::new(self.rpc.clone(), u64::MAX)) }}')
            else:
                o.append(f'        self.rpc.call::<{r0}>("ServicesHandle", 0, "{f}", {argv}).unwrap_or_else(|e| <{r0} as Lost>::lost(&e))')
            # the dispatcher
            tys, exs = [], []
            for i, (n, t) in enumerate(sent):
                ow, ex = owned(t); tys.append(ow); exs.append(ex.replace('x', f'x{i}', 1) if ex.startswith('x') else ex.replace('x', f'x{i}'))
            # a dropped audit callback is passed by the core
            call_args = []
            si = 0
            for n, t in ps:
                if envm and env_expr(t): call_args.append(env_expr(t))
                elif drop(t): call_args.append('root.audit.clone()')
                else: call_args.append(exs[si]); si += 1
            body = [f'            "{f}" => {{']
            if envm: body.append('                let env = root.env()?;')
            if sent:
                body.append('                let (' + ''.join(f'x{i}, ' for i in range(len(sent))) + '): (' + ''.join(t + ', ' for t in tys) + ') = de(a)?;')
            call = f't.{f}(' + ', '.join(call_args) + ')'
            if k == 'handle':
                body.append(f'                {{ let h = {call}; ser(&reg.add(Obj::{sub}(h))) }}')
            else:
                body.append(f'                ser(&{call})')
            body.append('            }')
            serve += body
        else:  # quiet
            o.append(f'        let _ = ({"".join(n + ", " for n in keep_names)});')
            o.append(f'        <{ret or "()"} as Lost>::lost(NOT_SERVED)')
        o.append('    }')
    o.append('}')
    o += ['', '/// The core\'s end of `ServicesHandle`: the plain methods and the handle accessors served over the socket.',
          "pub(crate) fn serve_services(root: &ServicesRoot, reg: &Registry<'_>, method: &str, a: serde_json::Value) -> Result<serde_json::Value, String> {",
          '    let t: &dyn ServicesHandle = root.svc.as_ref();', '    match method {']
    o += serve
    o += ['        "set_sound_settings" => {', '            let (v,): (iyakki_shared::audioout::SoundSettings,) = de(a)?;', '            ser(&t.update_sound_with(&mut |s| *s = v.clone()))', '        }',
          '        "set_system_sounds" => {', '            let (v,): (SystemSounds,) = de(a)?;', '            ser(&t.update_system_sounds_with(&mut |s| *s = v.clone()))', '        }',
          '        "router_plan_rows" => {', '            let (x0,): (String,) = de(a)?;', '            ser(&t.router_plan_rows(&x0))', '        }',
          '        "dir" => ser(&t.dir().to_path_buf()),', '        "cloud_stream_prompt_rule" => ser(&t.cloud_stream_prompt_rule()),', '        "fleet_prompt_rule" => ser(&t.fleet_prompt_rule()),',
          '        other => crate::served::serve_manual(root, reg, other, a),', '    }', '}']
    return '\n'.join(o) + '\n'

if __name__ == '__main__':
    txt = build()
    if '--check' in sys.argv:
        if open(OUT).read() != txt: print('gen_services.rs is stale: run dist/split/gen_services.py'); sys.exit(1)
        print('fresh'); sys.exit(0)
    open(OUT, 'w').write(txt); print('wrote', OUT)

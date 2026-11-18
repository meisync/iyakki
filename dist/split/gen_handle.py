#!/usr/bin/env python3
"""gen_handle.py name1,name2,...  (run in the repo root)
For methods of `Services` (inherent `pub fn`, or `fn` of an `impl ServicesB* for Services`) writes the declaration for `ServicesHandle`
(iyakki-agent-api/src/facets_h.rs) to /tmp/gen_h/api.rs and the forwarding impl (iyakki-agent/src/facets_h.rs) to /tmp/gen_h/impl.rs.
`self: &Arc<Self>` receivers become `&self` (the impl calls them on `self.arc()`). Unknown names are listed as MISSING; generic methods as GENERIC."""
import re,sys,os,glob
names=[n for n in sys.argv[1].split(',') if n]
files={}
for p in glob.glob('crates/iyakki-agent/src/**/*.rs',recursive=True):
    if p.endswith('_tests.rs') or '/tests' in p: continue
    s=open(p).read()
    if re.search(r'impl (?:crate::services::)?Services\b|for (?:crate::services::)?Services\b|impl iyakki_agent_api::\w+ for (?:crate::services::)?Services\b',s): files[p]=s
shared=set(re.findall(r'pub mod (\w+);',open('crates/iyakki-shared/src/lib.rs').read()))
def fix_types(t):
    def r(m):
        mod=m.group(1)
        return ('iyakki_shared::'+mod+'::') if mod in shared else m.group(0)
    t=re.sub(r'crate::(\w+)::',r,t)
    return t
def split_args(a):
    out=[];d=0;cur='';i=0
    while i<len(a):
        c=a[i]
        if c=='-' and a[i+1:i+2]=='>': cur+='->'; i+=2; continue
        if c in '<([': d+=1
        elif c in '>)]': d-=1
        if c==',' and d==0: out.append(cur.strip()); cur=''
        else: cur+=c
        i+=1
    if cur.strip(): out.append(cur.strip())
    return out
os.makedirs('/tmp/gen_h',exist_ok=True)
api='';imp='';miss=[];gen=[]
for n in names:
    found=None
    for p,s in files.items():
        for blk in re.finditer(r'\nimpl (?:crate::services::)?Services \{|\nimpl (?:iyakki_agent_api::)?(\w+) for (?:crate::services::)?Services \{|\nimpl iyakki_shared::\w+::\w+ for Services \{',s):
            start=blk.end()
            # block ends at the first "\n}\n" after start
            end=s.find('\n}\n',start)
            body=s[start:end]
            m=re.search(r'\n    (?:pub(?:\(crate\))? )?fn '+n+r'(<[^>]*>)?\(\s*(&self|self: &Arc<Self>|self: &Arc<Services>|&mut self)((?:[^{;]*?))\)( -> [^{;]*?)?\s*(?:where [^{]*)?\s*\{',body)
            if m: found=(blk.group(1),m); break
        if found: break
    if not found: miss.append(n); continue
    trait,m=found
    g=m.group(1) or ''
    if g: gen.append(n)
    args=re.sub(r'\s+',' ',m.group(3)).strip().lstrip(',').strip()
    ret=fix_types((m.group(4) or '').strip())
    parts=[p for p in split_args(args) if p.strip()] if args else []
    parts=[fix_types(p) for p in parts]
    pn=[re.sub(r'^mut\s+','',p.split(':')[0].strip()) for p in parts]
    arcr = 'Arc' in m.group(2)
    sig='&self'+(', '+', '.join(parts) if parts else '')
    api+='    fn %s%s(%s)%s;\n'%(n,g,sig,(' '+ret) if ret else '')
    if trait and trait.startswith('Services'):
        call='iyakki_agent_api::%s::%s(%s, %s)'%(trait,n,'&self.arc()' if arcr else 'self',', '.join(pn))
        call=call.replace(', )',')')
    else:
        call=('self.arc().%s(%s)' if arcr else 'Services::%s(self, %s)')
        call=(call%(n,', '.join(pn))) if arcr else (call%(n,', '.join(pn)))
        call=call.replace(', )',')')
    imp+='    fn %s%s(%s)%s {\n        %s\n    }\n'%(n,g,sig,(' '+ret) if ret else '',call)
open('/tmp/gen_h/api.rs','w').write(api); open('/tmp/gen_h/impl.rs','w').write(imp)
print('MISSING',miss); print('GENERIC',gen)

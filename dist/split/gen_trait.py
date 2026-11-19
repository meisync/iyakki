#!/usr/bin/env python3
"""gen_trait.py FILE STRUCT TRAIT m1,m2,...  -> writes TRAIT.api.rs and TRAIT.impl.rs in /tmp/gen/"""
import re,sys,os
f,struct,trait,names=sys.argv[1],sys.argv[2],sys.argv[3],sys.argv[4].split(',')
t=open(f).read()
tr='pub trait %s: Send + Sync {\n'%trait
im='impl %s for %s {\n'%('iyakki_agent_api::'+trait,'crate::'+f.split('/')[-1][:-3]+'::'+struct)
for n in names:
    m=re.search(r'\n    (?:///[^\n]*\n    )*pub fn '+n+r'(<[^>]*>)?\(([^{]*?)\)( -> [^{]*?)? (?:where [^{]*)?\{',t)
    if not m: print('MISSING',n,file=sys.stderr); continue
    gen=m.group(1) or ''; args=re.sub(r'\s+',' ',m.group(2).strip()); ret=(m.group(3) or '').strip()
    parts=re.split(r',\s*(?![^<(\[]*[>)\]])',args)
    selfarg=parts[0]
    pn=[re.sub(r'^mut\s+','',p.split(':')[0].strip()) for p in parts[1:] if p.strip()]
    tr+='    fn %s%s(%s)%s;\n'%(n,gen,args,(' '+ret) if ret else '')
    call='%s::%s(%s)'%('crate::'+f.split('/')[-1][:-3]+'::'+struct,n,', '.join(['self']+pn))
    im+='    fn %s%s(%s)%s {\n        %s\n    }\n'%(n,gen,args,(' '+ret) if ret else '',call)
tr+='}\n'; im+='}\n'
os.makedirs('/tmp/gen',exist_ok=True)
open('/tmp/gen/%s.api.rs'%trait,'w').write(tr); open('/tmp/gen/%s.impl.rs'%trait,'w').write(im)
print(tr)

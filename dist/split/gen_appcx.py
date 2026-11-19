#!/usr/bin/env python3
import re,sys,os,glob
names=sys.argv[1].split(',')
src=''
for p in glob.glob('iyakki-agent/src/**/*.rs',recursive=True):
    s=open(p).read()
    if 'impl Services' in s or 'for Services' in s: src+='\n//FILE '+p+'\n'+s
out=''; missing=[]
for n in names:
    m=re.search(r'\n    pub fn '+n+r'(<[^>]*>)?\(\s*(?:&self|self: &Arc<Self>|self: &Arc<Services>)((?:[^{;]*?))\)( -> [^{;]*?)?\s*(?:where [^{]*)?\{',src)
    if not m: missing.append(n); continue
    gen=m.group(1) or ''; args=m.group(2).strip(); ret=(m.group(3) or '').strip()
    args=re.sub(r'\s+',' ',args).lstrip(',').strip()
    parts=[p for p in re.split(r',\s*(?![^<(\[]*[>)\]])',args) if p.strip()] if args else []
    pn=[re.sub(r'^mut\s+','',p.split(':')[0].strip()) for p in parts]
    sig='&self'+(', '+', '.join(parts) if parts else '')
    out+='    pub fn %s%s(%s)%s {\n        self.services.%s(%s)\n    }\n\n'%(n,gen,sig,(' '+ret) if ret else '',n,', '.join(pn))
print(out)
print('MISSING',missing,file=sys.stderr)

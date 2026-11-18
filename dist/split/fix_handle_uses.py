#!/usr/bin/env python3
"""fix_handle_uses.py: reads /tmp/h_err.txt (handle_errs.sh) and fixes the mechanical ones in place:
 E0615/E0609 (a Services field used as a field)  -> the accessor call  `.NAME` -> `.NAME()`
 `iyakki_agent_api::ServicesBn::NAME(&X, ...)`  -> `X.NAME(...)` (everywhere in the UI crates, tests too); prints the NAMEs for add_handle.py"""
import re,glob,sys
names=set()
pat=re.compile(r'(?:iyakki_agent_api::)?ServicesB[0-9]::(\w+)\(\s*&?(\*?[\w.]+?)\s*(?:,\s*|\))')
for p in glob.glob('crates/iyakki-apps/src/**/*.rs',recursive=True)+glob.glob('crates/iyakki-shell/src/**/*.rs',recursive=True):
    s=open(p).read()
    def r(m):
        recv=m.group(2).lstrip('*')
        names.add(m.group(1))
        return '%s.%s(%s'%(recv,m.group(1),'' if m.group(0).endswith(')') and not m.group(0).endswith(',') else '') + (')' if m.group(0).endswith(')') else '')
    t=pat.sub(r,s)
    if t!=s: open(p,'w').write(t)
# field -> accessor, by error position
fix=0
for l in open('/tmp/h_err.txt'):
    m=re.match(r'(crates/\S+?):(\d+):(\d+): error\[E0(?:615|609)\]: (?:attempted to take value of method `(\w+)`|no field `(\w+)`)',l)
    if not m: continue
    f,ln,col,n1,n2=m.groups(); n=n1 or n2
    L=open(f).read().split('\n'); i=int(ln)-1
    line=L[i]
    # first occurrence of .n not followed by ( at or after col-1 (the receiver start)
    mm=re.compile(r'\.'+n+r'\b(?!\()').search(line,max(0,int(col)-1))
    if not mm: mm=re.compile(r'\.'+n+r'\b(?!\()').search(line)
    if mm:
        L[i]=line[:mm.end()]+'()'+line[mm.end():]; open(f,'w').write('\n'.join(L)); fix+=1
print('fields fixed',fix); print('names',','.join(sorted(names)))

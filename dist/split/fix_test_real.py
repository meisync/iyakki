#!/usr/bin/env python3
"""fix_test_real.py ERRFILE: E0599 `no method named X found for ... ServicesHandle` -> `.real().X` and a `use iyakki_agent::facets_h::RealServices as _;`
in the file (top of a *_tests.rs file, or inside the enclosing test module)."""
import re,sys,collections
errs=collections.defaultdict(list)
for l in open(sys.argv[1]):
    m=re.match(r'(crates/\S+?):(\d+):(\d+): error\[E0599\]: no method named `(\w+)` found for \S+ `[^`]*ServicesHandle',l)
    if m: errs[m.group(1)].append((int(m.group(2)),int(m.group(3)),m.group(4)))
n=0
for f,es in errs.items():
    L=open(f).read().split('\n')
    # bottom-up so columns stay valid
    for ln,col,name in sorted(es,reverse=True):
        line=L[ln-1]; i=col-1
        if line[i:i+len(name)]!=name: continue
        if i>0 and line[i-1]=='.':
            L[ln-1]=line[:i-1]+'.real().'+line[i:]; n+=1
    txt='\n'.join(L)
    if 'RealServices' not in txt:
        use='use iyakki_agent::facets_h::RealServices as _;'
        first=min(l for l,_,_ in es)
        if f.endswith('_tests.rs') or f.endswith('/tests.rs') or '/tests/' in f:
            k=0
            while k<len(L) and L[k].startswith('//!'): k+=1
            L.insert(k,use)
        else:
            k=None
            for j in range(first-1,-1,-1):
                if re.match(r'\s*(pub(\(crate\))? )?mod \w+ \{',L[j]):
                    k=j;break
            if k is None: print('no module for',f); continue
            ind=re.match(r'\s*',L[k]).group(0)+'    '
            L.insert(k+1,ind+use)
    open(f,'w').write('\n'.join(L))
print('real() inserted',n)

# ---- second pass: type mismatches at an argument: wrap the argument
def arg_end(line,i):
    d=0
    while i<len(line):
        c=line[i]
        if c in '([{': d+=1
        elif c in ')]}':
            if d==0: return i
            d-=1
        elif c==',' and d==0: return i
        i+=1
    return None
mm=collections.defaultdict(list)
for l in open(sys.argv[1]):
    m=re.match(r'(crates/\S+?):(\d+):(\d+): error\[E0308\]: mismatched types: expected `(&Services|&Arc<Services>|Arc<Services>)`, found `(&?Arc<dyn ServicesHandle>)`',l)
    if m: mm[m.group(1)].append((int(m.group(2)),int(m.group(3)),m.group(4),m.group(5)))
k=0
for f,es in mm.items():
    L=open(f).read().split('\n')
    need_use=False
    for ln,col,exp,found in sorted(es,reverse=True):
        line=L[ln-1]; i=col-1; e=arg_end(line,i)
        if e is None: continue
        expr=line[i:e]
        inner=expr[1:] if expr.startswith('&') else expr
        if exp=='&Services': new=inner+'.real()' if not inner.startswith('*') else inner[1:]+'.real()'; need_use=True
        elif exp=='&Arc<Services>': new='&iyakki_agent::services::Services::from_handle(&'+inner+')' if not expr.startswith('&') else '&iyakki_agent::services::Services::from_handle(&'+inner+')'
        else: new='iyakki_agent::services::Services::from_handle(&'+inner+')' if not expr.startswith('&') else 'iyakki_agent::services::Services::from_handle('+expr+')'
        if exp=='&Arc<Services>' and not expr.startswith('&'): new='&iyakki_agent::services::Services::from_handle('+inner+')'
        L[ln-1]=line[:i]+new+line[e:]; k+=1
    txt='\n'.join(L)
    if need_use and 'RealServices' not in txt:
        use='use iyakki_agent::facets_h::RealServices as _;'
        first=min(l for l,_,_,_ in es)
        if f.endswith('_tests.rs') or f.endswith('/tests.rs') or '/tests/' in f:
            j=0
            while j<len(L) and L[j].startswith('//!'): j+=1
            L.insert(j,use)
        else:
            j=None
            for q in range(first-1,-1,-1):
                if re.match(r'\s*(pub(\(crate\))? )?mod \w+ \{',L[q]): j=q;break
            if j is not None:
                ind=re.match(r'\s*',L[j]).group(0)+'    '
                L.insert(j+1,ind+use)
    open(f,'w').write('\n'.join(L))
print('arguments wrapped',k)

# ---- third pass: a test that needs the concrete store behind an accessor: `.NAME()` -> `.real().NAME` (Services field)
STORES='clis|jobs|radio|packages|cluster|k3sdev|sessions|skills|runs|ollama|mcp'
def add_use(f,L,first):
    txt='\n'.join(L)
    if 'RealServices' in txt: return
    use='use iyakki_agent::facets_h::RealServices as _;'
    if f.endswith('_tests.rs') or f.endswith('/tests.rs') or '/tests/' in f:
        j=0
        while j<len(L) and L[j].startswith('//!'): j+=1
        L.insert(j,use)
    else:
        for q in range(first-1,-1,-1):
            if re.match(r'\s*(pub(\(crate\))? )?mod \w+ \{',L[q]):
                ind=re.match(r'\s*',L[q]).group(0)+'    '
                L.insert(q+1,ind+use); break
t=collections.defaultdict(list)
for l in open(sys.argv[1]):
    m=re.match(r'(crates/\S+?):(\d+):(\d+): error\[E0308\]: mismatched types: expected `&(Jobs|CliManager|Arc<K3sDev>|K3sDev)`, found `&Arc<dyn \w+>`',l)
    if m: t[m.group(1)].append((int(m.group(2)),int(m.group(3)),'arg'))
    m=re.match(r'(crates/\S+?):(\d+):(\d+): error\[E0599\]: no method named `\w+` found for struct `std::sync::Arc<\(dyn (?:CliApi|JobApi|RadioApi|PackageApi|SandboxCluster|DevCluster|SessionApi|SkillApi|RunApi|OllamaApi|FleetApi)',l)
    if m: t[m.group(1)].append((int(m.group(2)),int(m.group(3)),'meth'))
c=0
for f,es in t.items():
    L=open(f).read().split('\n')
    for ln,col,kind in sorted(es,reverse=True):
        line=L[ln-1]; i=col-1
        if kind=='arg':
            e=arg_end(line,i)
            if e is None: continue
            expr=line[i:e]
            new=re.sub(r'\.('+STORES+r')\(\)$',r'.real().\1',expr)
            if new!=expr: L[ln-1]=line[:i]+new+line[e:]; c+=1
        else:
            pre=line[:i-1]
            mm=re.search(r'\.('+STORES+r')\(\)$',pre)
            if mm: L[ln-1]=pre[:mm.start()]+'.real().'+mm.group(1)+line[i-1:]; c+=1
    add_use(f,L,min(l for l,_,_ in es))
    open(f,'w').write('\n'.join(L))
print('stores made concrete',c)

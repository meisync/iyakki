import re,os,sys,collections
pat=re.compile(sys.argv[1])
cnt=collections.Counter(); sites=[]
for d in ('iyakki-apps/src','iyakki-shell/src'):
    for r,_,fs in os.walk(d):
        for f in fs:
            if not f.endswith('.rs') or f.endswith('_tests.rs') or f=='tests.rs' or '/tests' in r+'/': continue
            p=os.path.join(r,f); L=open(p).read().split('\n')
            end=len(L)
            for i,l in enumerate(L):
                if l.startswith('#[cfg(test)]'):
                    end=i;break
            for i in range(end):
                for m in pat.finditer(L[i]):
                    cnt[m.group(0)]+=1; sites.append((p,i+1,L[i].strip()[:150]))
for k,v in cnt.most_common(60): print(v,k)
print(len(sites),'sites')
if len(sys.argv)>2:
    for s in sites: print('%s:%d: %s'%s)

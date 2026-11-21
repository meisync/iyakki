#!/usr/bin/env python3
"""unserved_for.py NAME...: the trait methods (of the traits in rpc-traits.txt) whose arguments or answer name one of the given types, as
`Trait::method` lines for rpc-unserved.txt (the proxy answers those `not served over the UI socket`)."""
import sys, re, os, importlib.util
HERE = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location('rpc_census', os.path.join(HERE, 'rpc_census.py'))
c = importlib.util.module_from_spec(spec); spec.loader.exec_module(c)
allow = {l.split('#')[0].strip() for l in open(HERE + '/rpc-traits.txt') if l.split('#')[0].strip()}
names = [n.split('::')[-1] for n in sys.argv[1:]]
for r in c.build():
    if r['trait'] not in allow: continue
    txt = r['args'] + ' -> ' + r['ret']
    for n in names:
        if re.search(r'\b' + re.escape(n) + r'\b', txt):
            print('%s::%s  # %s (%s)' % (r['trait'], r['fn'], n, 'ret' if re.search(r'\b' + n + r'\b', r['ret']) else 'arg')); break

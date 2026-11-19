#!/usr/bin/env python3
"""add_handle.py name1,name2,...: gen_handle.py, then insert the declarations before `// GEN-END` of facets_h.rs (api) and the impls before it in the agent."""
import subprocess,sys
subprocess.run([sys.executable,'dist/split/gen_handle.py',sys.argv[1]],check=True)
a=open('/tmp/gen_h/api.rs').read().replace('iyakki_agent_api::','crate::'); i=open('/tmp/gen_h/impl.rs').read()
for p,t in (('crates/iyakki-agent-api/src/facets_h.rs',a),('crates/iyakki-agent/src/facets_h.rs',i)):
    s=open(p).read()
    assert '    // GEN-END' in s
    s=s.replace('    // GEN-END',t+'    // GEN-END',1)
    open(p,'w').write(s)

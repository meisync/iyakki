#!/usr/bin/env python3
"""add_serde.py NAME[,NAME...]: add `serde::Serialize, serde::Deserialize` to the derive of each named public struct or enum of the data crates
(F-SPLIT-03 piece 3: one type at a time as a trait enters rpc-traits.txt). Prints what it changed and what it could not find or does not allow."""
import re, sys, glob, os
ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', '..'))
DIRS = ['iyakki-shared', 'iyakki-memx', 'iyakki-extagentx', 'iyakki-accounts', 'iyakki-clock', 'iyakki-sec', 'iyakki-core-api']
files = [f for d in DIRS for f in glob.glob(f'{ROOT}/crates/{d}/src/**/*.rs', recursive=True)]
for spec in sys.argv[1].split(','):
    hint, _, name = spec.rpartition('::')   # `usagelog::Totals`, or only `Totals` when the name is unique
    hit = 0
    for f in files:
        if hint and not re.search(r'/' + hint.replace('::', '/') + r'(\.rs|/mod\.rs)$', f): continue
        s = open(f).read()
        m = re.search(r'((?:^[ \t]*#\[[^\n]*\n|^[ \t]*///[^\n]*\n)*)^(pub (?:struct|enum) ' + re.escape(name) + r'\b)', s, flags=re.M)
        if not m: continue
        attrs = m.group(1)
        if 'Serialize' in attrs and 'Deserialize' in attrs: print('already', name, f); hit = 1; break
        dm = re.search(r'#\[derive\(([^)]*)\)\]', attrs)
        if dm:
            new = attrs.replace(dm.group(0), '#[derive(' + dm.group(1).rstrip().rstrip(',') + ', serde::Serialize, serde::Deserialize)]', 1)
        else:
            new = attrs + '#[derive(serde::Serialize, serde::Deserialize)]\n'
        s = s[:m.start()] + new + m.group(2) + s[m.end():]
        open(f, 'w').write(s); print('derived', name, os.path.relpath(f, ROOT)); hit = 1; break
    if not hit: print('NOT FOUND', name)

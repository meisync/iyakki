#!/usr/bin/env python3
"""List FEATURES.md rows at Impl 100 whose Test is 0 or 30 and whose Manual is not 'needed'
(so the gap is missing tests, not hardware). Usage: test-gap-audit.py [--all] [--area F-XXX]"""
import re, sys, collections
rows = []
for line in open(sys.argv[0].rsplit('/dist/', 1)[0] + '/FEATURES.md' if '/dist/' in sys.argv[0] else 'FEATURES.md'):
    if not line.startswith('| F-'):
        continue
    c = [x.strip() for x in line.split('|')[1:-1]]
    if len(c) < 9:
        continue
    m = re.match(r'(\d+)/(\d+)$', c[5])
    if not m:
        continue
    rows.append((c[0], c[1], int(m[1]), int(m[2]), c[6], c[7], c[8]))
want_all = '--all' in sys.argv
area = next((a for a in sys.argv[1:] if a.startswith('F-')), '')
out = [r for r in rows if r[2] == 100 and r[3] in (0, 30) and r[0].startswith(area)
       and (want_all or not r[4].startswith('needed'))]
by = collections.Counter(r[0].rsplit('-', 1)[0] for r in out)
for r in out:
    print(f"{r[0]}\t{r[2]}/{r[3]}\t{r[4][:12]}\ttests={'none' if not r[5] else r[5].count('`')//2}\t{r[1][:60]}")
print('--', len(out), 'rows;', ', '.join(f'{k}:{v}' for k, v in by.most_common()), file=sys.stderr)

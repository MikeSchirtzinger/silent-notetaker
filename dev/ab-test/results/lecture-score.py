#!/usr/bin/env python3
"""Score the continuous-lecture repro: which of the 28 numbered checkpoints made it
into the transcript, and how much of each checkpoint's sentence text survived."""
import json, re, sys

ref_raw = open('/Users/mike/dev/silent-notetaker/dev/ab-test/lecture.txt').read()
# Split reference into (checkpoint-number, sentence) pairs.
parts = re.split(r'Checkpoint ([a-z ]+?)\.', ref_raw)[1:]
NUMWORDS = {w: i + 1 for i, w in enumerate(
    'one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen nineteen twenty'.split())}
def num(w):
    w = w.strip()
    if w in NUMWORDS: return NUMWORDS[w]
    if w.startswith('twenty'):
        rest = w[len('twenty'):].strip()
        return 20 + (NUMWORDS.get(rest, 0))
    return None
ref = {num(parts[i]): parts[i + 1].strip() for i in range(0, len(parts), 2)}

segs = json.load(open(sys.argv[1] if len(sys.argv) > 1 else
                      '/Users/mike/dev/silent-notetaker/dev/ab-test/results/ab-nemotron-lecture.json'))
hyp = ' '.join(s['text'] for s in segs).lower()
hyp_words = set(re.sub(r'[^a-z0-9 ]', ' ', hyp).split())

def content_words(s):
    stop = set('the a an of and to in is that by but it its this with for from can be we never about same not are or'.split())
    return [w for w in re.sub(r'[^a-z0-9 ]', ' ', s.lower()).split() if w not in stop and len(w) > 2]

missing, partial = [], []
for n in sorted(ref):
    cw = content_words(ref[n])
    hit = sum(1 for w in cw if w in hyp_words)
    frac = hit / max(1, len(cw))
    flag = 'OK  ' if frac >= 0.8 else ('PART' if frac >= 0.4 else 'MISS')
    if flag == 'MISS': missing.append(n)
    if flag == 'PART': partial.append(n)
    print(f'cp{n:02d} {flag} {hit}/{len(cw)}  {ref[n][:70]}')
print(f'\n{len(ref)} checkpoints: {len(ref)-len(missing)-len(partial)} ok, {len(partial)} partial, {len(missing)} missing')
print('missing:', missing or 'none')
print('partial:', partial or 'none')
print(f'segments: {len(segs)}; "checkpoint" mentions in hyp: {hyp.count("checkpoint")}')

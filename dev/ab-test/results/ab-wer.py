#!/usr/bin/env python3
"""Approximate WER for the A/B stress test. Digit->spoken normalization tuned to this script's numbers."""
import json, re, sys

REF = " ".join([
    "the third quarter revenue came in at four point two million dollars up eighteen percent from last year",
    "right and we shipped version two point oh on the fifteenth of march three days ahead of schedule",
    "the budget for twenty twenty six is capped at seven hundred fifty thousand dollars split across twelve teams",
    "our latency target is two hundred fifty milliseconds but we measured one hundred eighty seven on tuesday",
    "if you multiply three hundred sixty five by twenty four you get eight thousand seven hundred sixty hours in a year",
    "the conference call is at three thirty pm and the dial in code is four thousand four hundred twenty two",
    "headcount grows from forty eight to sixty four engineers in quarter four",
    "and remember the server bill was nine thousand eight hundred seventy six dollars and fifty four cents last month",
])

# Ordered digit->spoken rewrites (longest/most specific first). Applied to hypothesis after joining segments.
REWRITES = [
    (r"\$4\.\s*2 million", "four point two million dollars"),
    (r"\$9,?876\.\s*", "nine thousand eight hundred seventy six dollars "),  # then "54 last month" -> cents handled below
    (r"\$750,?000", "seven hundred fifty thousand dollars"),
    (r"\$76\.\s*54", "seventy six dollars and fifty four cents"),
    (r"\$76", "seventy six dollars"),
    (r"\$7\b", "seven"),
    (r"\b2\.\s*0\b", "two point oh"),
    (r"\b3\.\s*30\s*p\.?\s*m\.?", "three thirty pm"),
    (r"\b15th\b", "fifteenth"),
    (r"\b2026\b", "twenty twenty six"),
    (r"\b8,?760\b", "eight thousand seven hundred sixty"),
    (r"\b4,?422\b", "four thousand four hundred twenty two"),
    (r"\b9,?876\b", "nine thousand eight hundred seventy six"),
    (r"\b365\b", "three hundred sixty five"),
    (r"\b250\b", "two hundred fifty"),
    (r"\b187\b", "one hundred eighty seven"),
    (r"\b750\b", "seven hundred fifty"),
    (r"\b54\b", "fifty four"),
    (r"\b48\b", "forty eight"),
    (r"\b64\b", "sixty four"),
    (r"\b24\b", "twenty four"),
    (r"\b18%", "eighteen percent"),
    (r"\b18\b", "eighteen"),
    (r"\b12\b", "twelve"),
    (r"\b4\b", "four"),
    (r"%", " percent"),
]

def norm(text):
    t = text.lower()
    for pat, rep in REWRITES:
        t = re.sub(pat, rep, t)
    t = re.sub(r"[^a-z ]", " ", t)        # strip remaining punctuation
    t = re.sub(r"\s+", " ", t).strip()
    return t.split()

def wer(ref, hyp):
    d = [[0] * (len(hyp) + 1) for _ in range(len(ref) + 1)]
    for i in range(len(ref) + 1): d[i][0] = i
    for j in range(len(hyp) + 1): d[0][j] = j
    for i in range(1, len(ref) + 1):
        for j in range(1, len(hyp) + 1):
            d[i][j] = min(d[i-1][j] + 1, d[i][j-1] + 1,
                          d[i-1][j-1] + (ref[i-1] != hyp[j-1]))
    return d[len(ref)][len(hyp)]

ref_words = norm(REF)
print(f"reference: {len(ref_words)} words\n")
for name, path in [("nemotron-1x", "/tmp/ab-nemotron-1x-gated.json"),
                   ("nemotron-2x", "/tmp/ab-nemotron-2x-gated.json"),
                   ("voxtral-1x", "/tmp/ab-voxtral-1x.json"),
                   ("voxtral-2x", "/tmp/ab-voxtral-2x.json")]:
    segs = json.load(open(path))
    hyp = norm(" ".join(s["text"] for s in segs))
    e = wer(ref_words, hyp)
    print(f"{name:14s} WER ~{100*e/len(ref_words):5.1f}%  ({e} edits, {len(hyp)} hyp words, {len(segs)} segments)")

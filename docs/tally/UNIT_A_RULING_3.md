# Owner ruling 3 — two-dimensional outstandings segments

**Date:** 2026-07-31. **Supersedes R1 of `UNIT_A_RULING_2.md`.** R2 and R3 remain unchanged.

## Decision

AlterID-only segmentation is retracted. Every wildcard outstandings segment must carry both:

1. a narrow `SVFROMDATE` / `SVTODATE` window; and
2. an AlterID predicate `$AlterID > N AND $AlterID <= M` inside that window.

The 20-second deadline, wildcard fetch shape, paired-read completeness rules, no-retry rule,
40 MiB sealed response cap and 28 MiB target remain unchanged.

## Evidence

Measured on Aarav using the exact wildcard outstandings request:

| Date window | AlterID span | Time | Rows |
| --- | --- | ---: | ---: |
| Whole book | `0..400` | 31.5 s | 147 |
| One day | `0..400` | 0.7 s | 3 |
| One day | `0..25,000` | 69.3 s | 397 |
| One day | `0..50,000` | 41.9 s | 800 |

The date predicate makes the filter cheap. Cost follows the AlterID span scanned rather than
the number of rows returned, and the timings are non-monotonic. A single completed sample is
therefore not calibration evidence.

## Corpus ruling

`ALTVCHID` is 101,601, while the 1,632 vouchers dated `20240401` are spread across essentially
the whole AlterID space because Aarav's bulk generator inserted vouchers out of date order.
Aarav remains valid for protocol, completeness and failure-mode checks, but it must not tune a
segment-sizing policy.

The production segment width remains explicitly **uncalibrated** until the purpose-built
bill-bearing corpus exists and is generated strictly in ascending date order. Until then, the
screen must return a typed Partial without issuing a wildcard segment request. The ignored
reconciliation exit check remains blocked and unweakened.

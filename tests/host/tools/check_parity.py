#!/usr/bin/env python3
"""
Bit-level parity checker for BAMBOO weak-classifier fingerprints.

Input is a JSON fixture produced by:
  cargo run --manifest-path tests/host/Cargo.toml --bin export_parity_fixture -- --out <path>
"""

from __future__ import annotations

import argparse
import json
import sys
from typing import Any


def compute_fingerprint(elements: list[int], classifiers: list[dict[str, Any]]) -> int:
    fp = 0
    for clf in classifiers:
        pos = clf["positive_mask"]
        neg = clf["negative_mask"]
        threshold = int(clf["threshold"])
        max_iterations = min(len(elements), len(pos), len(neg))

        score = 0
        for i in range(max_iterations):
            score += (elements[i] & pos[i]).bit_count()
            score -= (elements[i] & neg[i]).bit_count()

        bit = 1 if score > threshold else 0
        fp = (fp << 1) | bit
    return fp


def main() -> int:
    parser = argparse.ArgumentParser(description="Check Python vs Rust fingerprint parity.")
    parser.add_argument("fixture", help="Path to parity fixture JSON")
    parser.add_argument(
        "--max-print",
        type=int,
        default=10,
        help="Max mismatches to print (default: 10)",
    )
    args = parser.parse_args()

    with open(args.fixture, "r", encoding="utf-8") as fh:
        fixture = json.load(fh)

    classifiers = fixture["classifiers"]
    samples = fixture["samples"]

    mismatches: list[tuple[int, dict[str, Any], int]] = []
    for idx, sample in enumerate(samples):
        py_fp = compute_fingerprint(sample["elements_bytes"], classifiers)
        rust_fp = int(sample["rust_fingerprint"])
        if py_fp != rust_fp:
            mismatches.append((idx, sample, py_fp))

    print(
        f"Model={fixture.get('model_version', '?')} samples={len(samples)} "
        f"mismatches={len(mismatches)}"
    )

    if mismatches:
        print("Parity failed. First mismatches:")
        for idx, sample, py_fp in mismatches[: args.max_print]:
            print(
                f"- sample={idx} device={sample['device']} file={sample['file']} "
                f"probe_idx={sample['probe_index_in_file']} rust={sample['rust_fingerprint']} "
                f"python={py_fp}"
            )
        return 1

    print("Parity OK: Python and Rust fingerprints match bit-for-bit.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Translate only the topic segment of K_<topic>_<discipline> TSV keys."""

from __future__ import annotations

import argparse
import re
import sqlite3
import unicodedata
from pathlib import Path

import ctranslate2
import sentencepiece as spm


CJK_RE = re.compile(r"[\u3400-\u4dbf\u4e00-\u9fff\uf900-\ufaff]")
SPACE_RE = re.compile(r"\s+")


def normalize(text: str) -> str:
    text = unicodedata.normalize("NFKC", text)
    text = SPACE_RE.sub(" ", text).strip()
    return text.casefold()


def split_key(key: str) -> tuple[str, str]:
    if not key.startswith("K_") or "_" not in key[2:]:
        raise ValueError(f"invalid topic key: {key!r}")
    return key[2:].rsplit("_", 1)


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("input", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--cache", type=Path, required=True)
    parser.add_argument("--model-dir", type=Path, required=True)
    parser.add_argument("--batch-size", type=int, default=4096)
    return parser.parse_args()


def main() -> None:
    args = arguments()
    rows: list[tuple[str, str, str]] = []
    topics: set[str] = set()
    with args.input.open(encoding="utf-8-sig") as source:
        for number, raw in enumerate(source, 1):
            key, separator, value = raw.rstrip("\n").partition("\t")
            if not separator:
                raise ValueError(f"line {number}: expected two tab-separated columns")
            topic, discipline = split_key(key)
            rows.append((topic, discipline, value))
            topics.add(topic)

    db = sqlite3.connect(args.cache)
    db.execute("PRAGMA journal_mode=WAL")
    db.execute("CREATE TABLE IF NOT EXISTS translations (source TEXT PRIMARY KEY, target TEXT NOT NULL)")
    known = dict(db.execute("SELECT source, target FROM translations"))

    plain = sorted(t for t in topics if not CJK_RE.search(t) and t not in known)
    db.executemany("INSERT OR REPLACE INTO translations VALUES (?, ?)", ((t, normalize(t)) for t in plain))
    db.commit()
    known.update((t, normalize(t)) for t in plain)

    pending = sorted(t for t in topics if CJK_RE.search(t) and t not in known)
    if pending:
        print(f"loading {args.model_dir} on CPU; {len(pending)} topics pending", flush=True)
        tokenizer = spm.SentencePieceProcessor(model_file=str(args.model_dir / "sentencepiece.model"))
        model = ctranslate2.Translator(
            str(args.model_dir / "model"),
            device="cpu",
            compute_type="int8",
            inter_threads=1,
        )

        for start in range(0, len(pending), args.batch_size):
            batch = pending[start : start + args.batch_size]
            encoded = [tokenizer.encode(text, out_type=str) for text in batch]
            generated = model.translate_batch(
                encoded,
                beam_size=1,
                max_decoding_length=64,
                batch_type="tokens",
                max_batch_size=4096,
            )
            translated = [normalize(tokenizer.decode(x.hypotheses[0]).replace("▁", " ")) for x in generated]
            translated = [target or normalize(source) for source, target in zip(batch, translated)]
            pairs = list(zip(batch, translated))
            db.executemany("INSERT OR REPLACE INTO translations VALUES (?, ?)", pairs)
            db.commit()
            known.update(pairs)
            done = min(start + len(batch), len(pending))
            print(f"translated {done}/{len(pending)}", flush=True)

    with args.output.open("w", encoding="utf-8", newline="") as destination:
        for topic, discipline, value in rows:
            destination.write(f"K_{known[topic]}_{discipline}\t{value}\n")
    db.close()


if __name__ == "__main__":
    main()

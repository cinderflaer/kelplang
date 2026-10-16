#!/usr/bin/env python3
"""Adds the index rows that are missing for fixtures, decided by the llvm backend.

The C oracle that froze `index.tsv` is gone, so the rows for fixtures added
after the freeze are decided here, the way the removed `oracle_freeze` test
decided them, with `kelpc build` (the default backend) in place of the C one:

* a target that `kelpc build` rejects is recorded as `c-refused` with the
  first two `": "` parts of its first stderr line, and has no golden;
* a fixture that builds is run with no arguments and stdin closed; it is
  recorded as `ran` with its exit status, the first stderr line and the
  SHA-256 of its stdout, and its stdout is written to `<tag>.out`;
* a build killed by a signal, or a run killed by a signal, stops the script
  and nothing is written, the way the freeze stopped.

Existing rows are kept byte for byte. Only fixtures with no row are added.
The header counts are recomputed from the rows.

The corpus is the one `golden_corpus.rs` enumerates: fixtures (`.kp` files in
`tests/fixtures`), projects (directories with a `coral.toml` in `tests/projects`,
built from the directory) and examples (`.kp` files in `examples`). A missing
row is added next to the nearest row before it in corpus order, so no existing
line moves.

Usage (from the repository root, after `cargo build -p kelpc`):

    python3 kelpc/tests/golden/regen_llvm.py target/debug/kelpc
"""

import hashlib
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
CRATE = os.path.abspath(os.path.join(HERE, "..", ".."))
FIXTURES = os.path.join(CRATE, "tests", "fixtures")
INDEX = os.path.join(HERE, "index.tsv")
COLUMNS = "tag\tstatus\texit\tstderr_first_line\tstdout_sha256"


def first_line(data):
    text = data.decode("utf-8", "replace")
    lines = text.splitlines()
    line = lines[0] if lines else ""
    return line.replace("\t", " ").replace("\r", " ").rstrip()


def refusal_kind(stderr):
    return ": ".join(first_line(stderr).split(": ")[:2])


def main():
    if len(sys.argv) != 2:
        sys.exit("usage: regen_llvm.py PATH_TO_KELPC")
    kelpc = os.path.abspath(sys.argv[1])

    with open(INDEX, encoding="utf-8") as f:
        lines = f.read().split("\n")
    if lines and lines[-1] == "":
        lines.pop()

    header_comment = [ln for ln in lines if ln.startswith("# kelpc golden set")]
    counts_idx = [i for i, ln in enumerate(lines) if ln.startswith("# counts ")]
    cols_idx = lines.index(COLUMNS)
    rows = [ln for ln in lines[cols_idx + 1:] if ln]
    known = {ln.split("\t")[0] for ln in rows}

    corpus = []  # (tag, source) in the corpus order of golden_corpus.rs
    for stem in sorted(n[:-3] for n in os.listdir(FIXTURES) if n.endswith(".kp")):
        corpus.append((f"fixture_{stem}", os.path.join(FIXTURES, stem + ".kp")))
    projects = os.path.join(CRATE, "tests", "projects")
    for name in sorted(os.listdir(projects)):
        path = os.path.join(projects, name)
        if os.path.isfile(os.path.join(path, "coral.toml")):
            corpus.append((f"project_{name}", path))
    examples = os.path.join(CRATE, "tests", "fixtures", "examples")
    for stem in sorted(n[:-3] for n in os.listdir(examples) if n.endswith(".kp")):
        corpus.append((f"example_{stem}", os.path.join(examples, stem + ".kp")))

    missing = [(t, src) for t, src in corpus if t not in known]
    print(f"{len(missing)} target(s) have no row")

    scratch = os.path.join(CRATE, "..", "..", "target", "golden_regen")
    os.makedirs(scratch, exist_ok=True)
    new_rows = []
    decided = {}
    for tag, source in missing:
        binary = os.path.join(scratch, tag)
        env = dict(os.environ)
        env.pop("KELPC_BACKEND", None)
        env.pop("KELPC_ORACLE", None)
        built = subprocess.run(
            [kelpc, "build", source, "-o", binary],
            env=env, capture_output=True,
        )
        if built.returncode < 0:
            sys.exit(f"'{tag}': the build was killed by signal {-built.returncode}; nothing written")
        if built.returncode != 0:
            kind = refusal_kind(built.stderr)
            print(f"{tag}: c-refused ({kind})")
            decided[tag] = f"{tag}\tc-refused\t\t{kind}\t"
            continue

        try:
            ran = subprocess.run(
                [binary], stdin=subprocess.DEVNULL, capture_output=True, timeout=60,
            )
        except subprocess.TimeoutExpired:
            sys.exit(f"'{tag}': the binary did not finish in 60 seconds; nothing written")
        if ran.returncode < 0:
            sys.exit(f"'{tag}': the binary was killed by signal {-ran.returncode}; nothing written")
        digest = hashlib.sha256(ran.stdout).hexdigest()
        with open(os.path.join(HERE, f"{tag}.out"), "wb") as f:
            f.write(ran.stdout)
        first = first_line(ran.stderr)
        print(f"{tag}: exit {ran.returncode} ({len(ran.stdout)} bytes)")
        decided[tag] = f"{tag}\tran\texit {ran.returncode}\t{first}\t{digest}"

    # Insert each new row after the nearest earlier corpus target that has a row.
    all_rows = list(rows)
    position = {ln.split("\t")[0]: i for i, ln in enumerate(all_rows)}
    anchor = None
    for tag, _ in corpus:
        if tag in decided:
            if anchor is None:
                at = 0
            else:
                at = position[anchor] + 1
            all_rows.insert(at, decided[tag])
            position = {ln.split("\t")[0]: i for i, ln in enumerate(all_rows)}
            anchor = tag
        elif tag in position:
            anchor = tag
    new_rows = list(decided.values())

    def count(status):
        return sum(1 for r in all_rows if r.split("\t")[1] == status)

    counts = (
        f"# counts candidates={len(all_rows)} ran={count('ran')} "
        f"c_refused={count('c-refused')} excluded={count('excluded')}"
    )
    out = list(header_comment)
    out.append(counts)
    out.append(COLUMNS)
    out.extend(all_rows)
    with open(INDEX, "w", encoding="utf-8") as f:
        f.write("\n".join(out) + "\n")
    print(f"index: {len(all_rows)} rows ({len(new_rows)} added)")


if __name__ == "__main__":
    main()
